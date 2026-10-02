//! The decision layer: what stands between the engine and the decision
//! model, so the server never depends on one and gains from one.
//!
//! * **The agent's questions** (`decide`, `wait_for` until, `pick`) always
//!   go to the model; an error is the agent's to see.
//! * **The server's own questions** ([`Use::Auto`]: an `expect`ed text in
//!   other words, the parts a `get_app_state(about)` means, the tool a
//!   `find_tools` query means) go only when they can help: a model is set
//!   up, `decision.auto` is on, it has been answering, and it answers
//!   within `decision.auto_timeout_ms`. Otherwise the caller judges by
//!   itself, as it did before there was a model, so nothing waits on one.
//! * **The same question about the same state is answered once**
//!   (`decision.cache_seconds`): waits that ask again, a look at the same
//!   window, a pick repeated, cost nothing the second time.
//! * **A model that keeps failing is left alone** for a minute: three
//!   failures in a row and the server's own questions stop going to it.
//! * **What it cost is counted**: requests, answers from the cache, time,
//!   and the text the model read so the agent didn't have to.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use super::{Answers, Asked, Decider, Question};
use crate::config::DecisionConfig;
use crate::error::Error;

/// Failures in a row after which the server's own questions stop.
const TRIP_AFTER: u32 = 3;
/// How long they stay stopped.
const COOL_DOWN: Duration = Duration::from_secs(60);
/// The most answers kept.
const CACHE_ENTRIES: usize = 512;

/// Who wants the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    /// The agent asked: always tried.
    Asked,
    /// The server asks on its own, to save the agent a turn or a read:
    /// only while the model is healthy, and briefly.
    Auto,
}

/// What the decision layer did, since the server started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    /// Requests sent to the model.
    pub requests: u64,
    /// Of them, the server's own.
    pub auto: u64,
    /// Answers given from the cache, without a request.
    pub cached: u64,
    /// Requests that failed.
    pub failed: u64,
    /// The server's own questions not asked (the model was failing).
    pub skipped: u64,
    /// Time spent waiting for answers (ms).
    pub waited_ms: u64,
    /// Characters of state the model read (the agent didn't have to).
    pub chars_read: u64,
}

/// The decision layer's memory: answers, failures and counts.
#[derive(Debug, Default)]
pub struct Judge {
    answers: HashMap<u64, (Instant, Answers)>,
    order: VecDeque<u64>,
    failures: u32,
    resting_until: Option<Instant>,
    stats: Stats,
}

impl Judge {
    /// Whether the server's own questions may go to the model now.
    pub fn ready(&self, now: Instant) -> bool {
        self.resting_until.is_none_or(|t| now >= t)
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Questions about states, each its own request: the ones answered
    /// lately from the cache, the others sent together. The server's own
    /// questions get `auto_timeout_ms`, and none at all while the model
    /// rests after failing.
    pub fn ask_batch(
        &mut self,
        decider: &Decider,
        settings: &DecisionConfig,
        use_: Use,
        jobs: &[(String, Vec<Question>)],
        now: Instant,
        halted: &(dyn Fn() -> bool + Sync),
    ) -> Vec<Asked> {
        if use_ == Use::Auto && !self.ready(now) {
            self.stats.skipped += jobs.len() as u64;
            return jobs
                .iter()
                .map(|_| {
                    Err(Error::ActionFailed(
                        "the decision model is resting after failing".into(),
                    ))
                })
                .collect();
        }
        let keep = Duration::from_secs(settings.cache_seconds);
        let keys: Vec<u64> = jobs.iter().map(|(s, q)| key(decider, s, q)).collect();
        let mut out: Vec<Option<Asked>> = keys
            .iter()
            .map(|k| {
                let (at, answers) = self.answers.get(k)?;
                (!keep.is_zero() && now.saturating_duration_since(*at) < keep)
                    .then(|| Ok((answers.clone(), Duration::ZERO)))
            })
            .collect();
        self.stats.cached += out.iter().filter(|o| o.is_some()).count() as u64;
        let missing: Vec<usize> = (0..jobs.len()).filter(|&i| out[i].is_none()).collect();
        if !missing.is_empty() {
            let d = match use_ {
                Use::Auto => {
                    decider.with_timeout(Duration::from_millis(settings.auto_timeout_ms.max(100)))
                }
                Use::Asked => decider.clone(),
            };
            let ask: Vec<(String, Vec<Question>)> =
                missing.iter().map(|&i| jobs[i].clone()).collect();
            let start = Instant::now();
            let replies = d.ask_batch(&ask, halted);
            self.stats.waited_ms += start.elapsed().as_millis() as u64;
            self.stats.requests += ask.len() as u64;
            if use_ == Use::Auto {
                self.stats.auto += ask.len() as u64;
            }
            for (&i, reply) in missing.iter().zip(replies) {
                match &reply {
                    Ok((answers, _)) => {
                        self.failures = 0;
                        self.resting_until = None;
                        self.stats.chars_read += jobs[i].0.chars().count() as u64;
                        if !keep.is_zero() {
                            self.remember(keys[i], now, answers.clone());
                        }
                    }
                    // A wrong question or the stop key says nothing about
                    // the model.
                    Err(Error::InvalidArgs(_)) => {}
                    Err(_) if halted() => {}
                    Err(_) => {
                        self.stats.failed += 1;
                        self.failures += 1;
                        if self.failures >= TRIP_AFTER {
                            self.resting_until = Some(now + COOL_DOWN);
                        }
                    }
                }
                out[i] = Some(reply);
            }
        }
        out.into_iter()
            .map(|o| o.unwrap_or_else(|| Err(Error::Internal("no answer".into()))))
            .collect()
    }

    fn remember(&mut self, key: u64, now: Instant, answers: Answers) {
        if self.answers.insert(key, (now, answers)).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > CACHE_ENTRIES {
            if let Some(old) = self.order.pop_front() {
                self.answers.remove(&old);
            }
        }
    }

    /// Forget every answer (the settings changed: another model).
    pub fn forget(&mut self) {
        self.answers.clear();
        self.order.clear();
        self.failures = 0;
        self.resting_until = None;
    }

    /// For `decide setup="status"`: what the layer did.
    pub fn report(&self, now: Instant) -> String {
        let s = &self.stats;
        if s.requests == 0 && s.cached == 0 && s.skipped == 0 {
            return "Not asked yet in this session.".into();
        }
        let mut out = format!(
            "This session: {} request(s) ({} by the server on its own), {} answer(s) from the cache",
            s.requests, s.auto, s.cached
        );
        if s.requests > 0 {
            out.push_str(&format!(
                ", {} ms a request on average",
                s.waited_ms / s.requests.max(1)
            ));
        }
        if s.failed > 0 {
            out.push_str(&format!(", {} failed", s.failed));
        }
        if s.skipped > 0 {
            out.push_str(&format!(", {} not asked while it rested", s.skipped));
        }
        out.push_str(&format!(
            "; it read ~{} tokens of state the agent didn't have to.",
            s.chars_read / 4
        ));
        if let Some(t) = self.resting_until.filter(|t| now < *t) {
            out.push_str(&format!(
                " It failed {TRIP_AFTER} times in a row: the server judges by itself for {} s more.",
                t.saturating_duration_since(now).as_secs()
            ));
        }
        out
    }
}

/// What tells one question apart from another: the model, the state and
/// the questions.
fn key(decider: &Decider, state: &str, questions: &[Question]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    decider.identity().hash(&mut h);
    state.hash(&mut h);
    questions.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::decision::tests::fake;

    fn jev(url: &str) -> (Decider, DecisionConfig) {
        let c = DecisionConfig {
            provider: "jev".into(),
            base_url: url.into(),
            api_key: "k".into(),
            ..DecisionConfig::default()
        };
        (Decider::from_config(&c).unwrap().unwrap(), c)
    }

    /// One question through the layer.
    fn ask(
        j: &mut Judge,
        (d, c): (&Decider, &DecisionConfig),
        use_: Use,
        state: &str,
        q: &[Question],
        now: Instant,
    ) -> Asked {
        j.ask_batch(d, c, use_, &[(state.into(), q.to_vec())], now, &|| false)
            .pop()
            .unwrap()
    }

    fn yes(p: f64) -> (u16, String) {
        (
            200,
            json!({"answers": {"a": {"type": "noul", "noul": p}}}).to_string(),
        )
    }

    #[test]
    fn answers_are_kept_for_a_while_and_by_question() {
        let f = fake(|_| yes(0.9));
        let (d, c) = jev(&f.url);
        let mut j = Judge::default();
        let t0 = Instant::now();
        let q = [Question::yes_no("a", "is it?")];
        for _ in 0..3 {
            let (a, _) = ask(&mut j, (&d, &c), Use::Asked, "state", &q, t0).unwrap();
            assert!(a[0].1.is_yes());
        }
        assert_eq!(f.seen.lock().unwrap().len(), 1);
        assert_eq!((j.stats().requests, j.stats().cached), (1, 2));
        // Another question, another state: asked.
        let other = [Question::yes_no("a", "is it really?")];
        ask(&mut j, (&d, &c), Use::Asked, "state", &other, t0).unwrap();
        ask(&mut j, (&d, &c), Use::Asked, "state 2", &q, t0).unwrap();
        assert_eq!(f.seen.lock().unwrap().len(), 3);
        // After cache_seconds: asked again.
        let later = t0 + Duration::from_secs(c.cache_seconds + 1);
        ask(&mut j, (&d, &c), Use::Asked, "state", &q, later).unwrap();
        assert_eq!(f.seen.lock().unwrap().len(), 4);
        // cache_seconds = 0: never kept.
        let none = DecisionConfig {
            cache_seconds: 0,
            ..c.clone()
        };
        let mut j = Judge::default();
        ask(&mut j, (&d, &none), Use::Asked, "s", &q, t0).unwrap();
        ask(&mut j, (&d, &none), Use::Asked, "s", &q, t0).unwrap();
        assert_eq!(f.seen.lock().unwrap().len(), 6);
    }

    #[test]
    fn the_servers_own_questions_rest_after_failures_and_come_back() {
        let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let o = ok.clone();
        let f = fake(move |_| {
            if o.load(std::sync::atomic::Ordering::SeqCst) {
                yes(0.8)
            } else {
                (503, "{}".into())
            }
        });
        let (d, c) = jev(&f.url);
        let mut j = Judge::default();
        let t0 = Instant::now();
        for i in 0..TRIP_AFTER {
            let q = [Question::yes_no("a", &format!("question {i}?"))];
            assert!(ask(&mut j, (&d, &c), Use::Auto, "s", &q, t0).is_err());
        }
        assert!(!j.ready(t0));
        let q = [Question::yes_no("a", "now?")];
        assert!(ask(&mut j, (&d, &c), Use::Auto, "s", &q, t0).is_err());
        assert_eq!(f.seen.lock().unwrap().len(), TRIP_AFTER as usize);
        assert_eq!(j.stats().skipped, 1);
        // After the rest, and once it answers, all is as before.
        ok.store(true, std::sync::atomic::Ordering::SeqCst);
        let later = t0 + COOL_DOWN;
        assert!(j.ready(later));
        assert!(ask(&mut j, (&d, &c), Use::Auto, "s", &q, later).is_ok());
        assert!(j.ready(later));
    }

    #[test]
    fn a_batch_keeps_its_order_and_asks_only_whats_missing() {
        let f = fake(|body| {
            let v: serde_json::Value = serde_json::from_str(body).unwrap();
            let n: f64 = v["state"].as_str().unwrap().parse().unwrap();
            yes(n / 10.0)
        });
        let (d, c) = jev(&f.url);
        let mut j = Judge::default();
        let t0 = Instant::now();
        let q = vec![Question::yes_no("a", "big?")];
        let jobs: Vec<(String, Vec<Question>)> =
            (0..6).map(|i| (i.to_string(), q.clone())).collect();
        ask(&mut j, (&d, &c), Use::Asked, "3", &q, t0).unwrap();
        let out = j.ask_batch(&d, &c, Use::Asked, &jobs, t0, &|| false);
        for (i, r) in out.iter().enumerate() {
            let (a, _) = r.as_ref().unwrap();
            assert_eq!(
                a[0].1,
                crate::decision::Answer::YesNo {
                    yes: i as f64 / 10.0
                }
            );
        }
        // "3" came from the cache: 1 + 5 requests.
        assert_eq!(f.seen.lock().unwrap().len(), 6);
    }
}
