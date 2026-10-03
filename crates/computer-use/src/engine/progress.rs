//! How far a long call has got, for a host that shows it (MCP's
//! `notifications/progress`): batch steps, a drawing, a wait, a script's
//! tool calls.

use std::sync::Mutex;

use super::*;

/// One report of a call's progress: `progress` only grows within a call,
/// `total` when it is known.
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub progress: f64,
    pub total: Option<f64>,
    pub message: String,
}

/// Where a call's progress goes (set by the host for one call).
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// Reports between these are dropped (but the first and the last).
const EVERY: Duration = Duration::from_millis(100);

/// A call's progress reports: never going back, and not more often than
/// [`EVERY`]. Cloned into loops that can't borrow the engine (a drawing's
/// pace).
#[derive(Clone)]
pub(super) struct Reporter {
    sink: ProgressSink,
    last: Arc<Mutex<(f64, Option<Instant>)>>,
}

impl Reporter {
    pub(super) fn new(sink: ProgressSink) -> Self {
        Self {
            sink,
            last: Arc::new(Mutex::new((f64::NEG_INFINITY, None))),
        }
    }

    /// Report `progress` (of `total`), unless it is no further than the
    /// last one, or comes too soon after it and isn't the end.
    pub(super) fn report(&self, progress: f64, total: Option<f64>, message: impl Into<String>) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if !progress.is_finite() || progress <= last.0 {
            return;
        }
        let now = Instant::now();
        let done = total.is_some_and(|t| progress >= t);
        if !done && last.1.is_some_and(|at| now.duration_since(at) < EVERY) {
            return;
        }
        *last = (progress, Some(now));
        drop(last);
        (self.sink)(Progress {
            progress,
            total,
            message: message.into(),
        });
    }
}

impl<B: Backend> Engine<B> {
    /// Send the progress of the calls that follow to `sink` (MCP hosts:
    /// when the request carries a `progressToken`); `None` stops it.
    pub fn set_progress(&mut self, sink: Option<ProgressSink>) {
        self.progress = sink.map(Reporter::new);
    }

    /// The reporter for the call in progress: only a top-level call
    /// reports (a batch's steps and a script's tools are its progress).
    pub(super) fn reporter(&self) -> Option<Reporter> {
        (self.ctx.depth == 1)
            .then(|| self.progress.clone())
            .flatten()
    }

    /// Report the top-level call's progress, if the host asked for it.
    pub(super) fn report_progress(
        &self,
        progress: f64,
        total: Option<f64>,
        message: impl FnOnce() -> String,
    ) {
        if let Some(r) = self.reporter() {
            r.report(progress, total, message());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect() -> (ProgressSink, Arc<Mutex<Vec<Progress>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink_got = got.clone();
        (Arc::new(move |p| sink_got.lock().unwrap().push(p)), got)
    }

    #[test]
    fn progress_only_grows_and_the_end_always_goes() {
        let (sink, got) = collect();
        let r = Reporter::new(sink);
        r.report(1.0, Some(3.0), "one");
        r.report(1.0, Some(3.0), "again");
        r.report(0.5, Some(3.0), "back");
        // Too soon after the first, and not the end.
        r.report(2.0, Some(3.0), "two");
        r.report(3.0, Some(3.0), "done");
        r.report(f64::NAN, None, "nan");
        let got: Vec<String> = got
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.message.clone())
            .collect();
        assert_eq!(got, ["one", "done"]);
    }
}
