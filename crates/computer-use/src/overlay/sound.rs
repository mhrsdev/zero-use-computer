//! Sounds for what the agent does (`overlay.sound`, off by default): a
//! click, keys pressed and text typed, the work done, an error, the agent
//! stopped. Played by the overlay's helper (or the hub), never by the agent.
//!
//! The built-in sounds are made here (short tones and ticks, written once as
//! WAV files into the cache folder, at the volume asked for: nothing to
//! license). The user may give a WAV file of their own for each, in the
//! settings; nothing else is ever played. The system's player plays them:
//! on Linux the first of `pw-play`, `paplay` and `aplay` on `PATH`, on macOS
//! `afplay`, on Windows `PlaySound`. A player is started with its arguments
//! (never through a shell) and never waited for; each sound is rate-limited
//! (typing doesn't rattle) and a sound that can't be played is skipped, said
//! once.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::OverlayConfig;

/// What a sound is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    Click,
    /// Keys pressed, text typed.
    Key,
    Done,
    Error,
    /// Stopped by the stop key or the stop button.
    Stopped,
}

impl Event {
    pub const ALL: [Event; 5] = [
        Event::Click,
        Event::Key,
        Event::Done,
        Event::Error,
        Event::Stopped,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Event::Click => "click",
            Event::Key => "key",
            Event::Done => "done",
            Event::Error => "error",
            Event::Stopped => "stopped",
        }
    }

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == name.trim())
    }

    /// The least time between two of these.
    fn gap(self) -> Duration {
        Duration::from_millis(match self {
            Event::Click => 80,
            Event::Key => 140,
            Event::Done | Event::Error | Event::Stopped => 700,
        })
    }

    /// Whether the settings want it, and their own file for it ("").
    fn wanted<'a>(self, cfg: &'a OverlayConfig) -> (bool, &'a str) {
        match self {
            Event::Click => (cfg.sound_click, &cfg.sound_click_file),
            Event::Key => (cfg.sound_key, &cfg.sound_key_file),
            Event::Done => (cfg.sound_done, &cfg.sound_done_file),
            Event::Error => (cfg.sound_error, &cfg.sound_error_file),
            Event::Stopped => (cfg.sound_stopped, &cfg.sound_stopped_file),
        }
    }
}

/// How a player is told the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    PwPlay,
    Paplay,
    Aplay,
    Afplay,
    /// Windows' own `PlaySound` (no program).
    Native,
}

/// The system's player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    path: PathBuf,
    kind: Kind,
}

impl Program {
    /// The player for this system: found in `path_var` (a `PATH` value)
    /// on Linux and macOS.
    pub fn find(path_var: Option<&OsStr>) -> Option<Program> {
        if cfg!(windows) {
            return Some(Program {
                path: PathBuf::from("PlaySound"),
                kind: Kind::Native,
            });
        }
        let names: &[(&str, Kind)] = if cfg!(target_os = "macos") {
            &[("afplay", Kind::Afplay)]
        } else {
            &[
                ("pw-play", Kind::PwPlay),
                ("paplay", Kind::Paplay),
                ("aplay", Kind::Aplay),
            ]
        };
        let dirs: Vec<PathBuf> = path_var.map(|p| std::env::split_paths(p).collect())?;
        names.iter().find_map(|(name, kind)| {
            dirs.iter()
                .map(|d| d.join(name))
                .find(|p| p.is_file())
                .map(|path| Program { path, kind: *kind })
        })
    }

    /// Its name, as people know it.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// The arguments that play `file` at `volume` (0–100; 100 plays it as
    /// it is).
    fn args(&self, file: &Path, volume: u32) -> Vec<OsString> {
        let v = f64::from(volume.min(100)) / 100.0;
        let mut a: Vec<OsString> = match self.kind {
            Kind::PwPlay => vec![format!("--volume={v:.2}").into()],
            Kind::Paplay => vec![format!("--volume={}", (v * 65536.0).round() as u32).into()],
            Kind::Aplay => vec!["-q".into()],
            Kind::Afplay => vec!["-v".into(), format!("{v:.2}").into()],
            Kind::Native => Vec::new(),
        };
        // Always a full path (the cache folder's, or one from the settings
        // made full): it never starts with a dash, so it isn't an option.
        a.push(file.as_os_str().to_owned());
        a
    }
}

/// Where the built-in sounds are written.
pub fn sound_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|d| d.join("zero-use-computer"))
        .unwrap_or_else(|| crate::config::home_dir().join("cache"))
        .join("sounds")
}

/// At most this many sounds a second, all events together.
const PER_SECOND: usize = 6;
/// Players running at once, at most; one that runs this long is stopped.
const AT_ONCE: usize = 3;
const LONGEST: Duration = Duration::from_secs(8);

/// Plays the sounds, at most so many, never waiting.
pub struct Player {
    dir: PathBuf,
    path_var: Option<OsString>,
    /// The system's player, once looked for.
    program: Option<Option<Program>>,
    last: HashMap<Event, Instant>,
    recent: VecDeque<Instant>,
    running: Vec<(Child, Instant)>,
    /// The built-in sounds written, at their volume.
    written: HashSet<(Event, u32)>,
    /// What was said already, and what is still to say (once each).
    said: HashSet<String>,
    warnings: Vec<String>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// The system's player, built-in sounds in [`sound_dir`].
    pub fn new() -> Self {
        Self::with(sound_dir(), std::env::var_os("PATH"))
    }

    /// Built-in sounds in `dir`, the player looked for in `path_var`.
    pub fn with(dir: PathBuf, path_var: Option<OsString>) -> Self {
        Self {
            dir,
            path_var,
            program: None,
            last: HashMap::new(),
            recent: VecDeque::new(),
            running: Vec::new(),
            written: HashSet::new(),
            said: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    /// What should be said once (in the log), since last asked.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn warn(&mut self, text: String) {
        if self.said.insert(text.clone()) {
            self.warnings.push(text);
        }
    }

    /// Play `event` as `cfg` asks (no sound when they are off, or it came
    /// too soon after the last). Returns the file handed to the player.
    pub fn play(&mut self, event: Event, cfg: &OverlayConfig, now: Instant) -> Option<PathBuf> {
        let (on, _) = event.wanted(cfg);
        if !cfg.sound || !on {
            return None;
        }
        self.start(event, cfg, now).ok()
    }

    /// Play `event` now whatever the switches say (the panel's test
    /// button): the player's name and the file, or why not.
    pub fn test(&mut self, event: Event, cfg: &OverlayConfig) -> Result<(String, PathBuf), String> {
        self.last.remove(&event);
        let file = self.start(event, cfg, Instant::now())?;
        let name = self
            .program
            .clone()
            .flatten()
            .map(|p| p.name())
            .unwrap_or_default();
        Ok((name, file))
    }

    fn start(
        &mut self,
        event: Event,
        cfg: &OverlayConfig,
        now: Instant,
    ) -> Result<PathBuf, String> {
        // Too soon after the same sound, or too many lately.
        if self
            .last
            .get(&event)
            .is_some_and(|t| now.saturating_duration_since(*t) < event.gap())
        {
            return Err("too soon after the last one".into());
        }
        while self
            .recent
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(1))
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= PER_SECOND {
            return Err("too many sounds at once".into());
        }
        self.reap(now);
        if self.running.len() >= AT_ONCE {
            return Err("too many sounds at once".into());
        }
        let program = match &self.program {
            Some(p) => p.clone(),
            None => {
                let p = Program::find(self.path_var.as_deref());
                self.program = Some(p.clone());
                p
            }
        };
        let Some(program) = program else {
            let e = "no sound player found (pw-play, paplay or aplay on Linux; afplay on macOS), so the overlay's sounds are silent".to_string();
            self.warn(e.clone());
            return Err(e);
        };
        let volume = cfg.sound_volume.min(100);
        let (_, own) = event.wanted(cfg);
        let own = crate::config::settings_path(Path::new(own.trim()));
        let (file, volume) = match own {
            Some(p) if p.is_file() => (p, volume),
            Some(p) => {
                self.warn(format!(
                    "the sound file {} can't be read; the built-in {} sound plays instead",
                    p.display(),
                    event.name()
                ));
                (self.builtin(event, volume)?, 100)
            }
            // Made at its volume: every player plays it as it is.
            None => (self.builtin(event, volume)?, 100),
        };
        self.last.insert(event, now);
        self.recent.push_back(now);
        if program.kind == Kind::Native {
            if let Err(e) = play_native(&file) {
                self.warn(format!("the sound can't be played: {e}"));
                return Err(e);
            }
            return Ok(file);
        }
        let mut cmd = Command::new(&program.path);
        cmd.args(program.args(&file, volume))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match cmd.spawn() {
            Ok(child) => {
                self.running.push((child, now));
                Ok(file)
            }
            Err(e) => {
                let e = format!("{} can't play sounds: {e}", program.path.display());
                self.warn(e.clone());
                Err(e)
            }
        }
    }

    /// Players done are let go; one that runs too long is stopped.
    fn reap(&mut self, now: Instant) {
        self.running.retain_mut(|(c, since)| {
            if let Ok(Some(_)) = c.try_wait() {
                return false;
            }
            if now.saturating_duration_since(*since) >= LONGEST {
                let _ = c.kill();
                let _ = c.wait();
                return false;
            }
            true
        });
    }

    /// The built-in sound for `event` at `volume`, written once.
    fn builtin(&mut self, event: Event, volume: u32) -> Result<PathBuf, String> {
        let file = self.dir.join(format!("{}-{volume}.wav", event.name()));
        if self.written.contains(&(event, volume)) && file.is_file() {
            return Ok(file);
        }
        let wav = wav(&synth(event), f32::from(volume as u16) / 100.0);
        let ok = std::fs::create_dir_all(&self.dir).and_then(|_| {
            // Written aside, then put in place: a player never reads half.
            let part = self.dir.join(format!(".{}-{volume}.part", event.name()));
            std::fs::write(&part, &wav)?;
            std::fs::rename(&part, &file)
        });
        match ok {
            Ok(()) => {
                self.written.insert((event, volume));
                Ok(file)
            }
            Err(e) => {
                let e = format!("can't write the sounds to {}: {e}", self.dir.display());
                self.warn(e.clone());
                Err(e)
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // Players still running finish by themselves; none is left a zombie.
        for (mut c, _) in self.running.drain(..) {
            let _ = std::thread::Builder::new()
                .name("sound-reap".into())
                .spawn(move || {
                    let _ = c.wait();
                });
        }
    }
}

#[cfg(windows)]
fn play_native(file: &Path) -> Result<(), String> {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let name = windows_core::HSTRING::from(file.as_os_str());
    // SAFETY: plays a file by name, at once returning (SND_ASYNC); the
    // name is copied by the call.
    let ok = unsafe { PlaySoundW(&name, None, SND_ASYNC | SND_FILENAME | SND_NODEFAULT) };
    if ok.as_bool() {
        Ok(())
    } else {
        Err(format!("Windows didn't play {}", file.display()))
    }
}

#[cfg(not(windows))]
fn play_native(_file: &Path) -> Result<(), String> {
    Err("no native player here".into())
}

const RATE: u32 = 44_100;

/// The built-in sound for `event`: samples −1…1 at 44.1 kHz.
fn synth(event: Event) -> Vec<f32> {
    use std::f32::consts::TAU;
    let n = |ms: f32| (ms / 1000.0 * RATE as f32) as usize;
    let tone = |out: &mut Vec<f32>,
                at: usize,
                len: usize,
                f0: f32,
                f1: f32,
                decay: f32,
                amp: f32,
                harm: f32| {
        if out.len() < at + len {
            out.resize(at + len, 0.0);
        }
        let mut phase = 0.0f32;
        for i in 0..len {
            let k = i as f32 / len as f32;
            let f = f0 + (f1 - f0) * k;
            phase += TAU * f / RATE as f32;
            // A soft start (no click), then a decay.
            let attack = (i as f32 / (RATE as f32 * 0.002)).min(1.0);
            let env = attack * (-decay * k).exp() * (1.0 - k).min(1.0);
            out[at + i] += amp * env * (phase.sin() + harm * (2.0 * phase).sin());
        }
    };
    let mut out = Vec::new();
    match event {
        Event::Click => {
            // A short tick: a little noise, then a high tone dying quickly.
            let mut seed = 0x2545_f491u32;
            out.resize(n(45.0), 0.0);
            for (i, v) in out.iter_mut().enumerate().take(n(4.0)) {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let r = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
                *v += 0.35 * r * (1.0 - i as f32 / n(4.0) as f32);
            }
            tone(&mut out, 0, n(45.0), 1900.0, 1500.0, 9.0, 0.55, 0.15);
        }
        Event::Key => tone(&mut out, 0, n(30.0), 2600.0, 2300.0, 6.0, 0.45, 0.1),
        Event::Done => {
            // Up a fifth, gently.
            tone(&mut out, 0, n(170.0), 880.0, 880.0, 3.5, 0.4, 0.2);
            tone(&mut out, n(110.0), n(320.0), 1318.5, 1318.5, 3.0, 0.4, 0.2);
        }
        Event::Error => {
            // Down, twice, lower.
            tone(&mut out, 0, n(160.0), 466.2, 440.0, 3.0, 0.45, 0.35);
            tone(&mut out, n(170.0), n(240.0), 349.2, 329.6, 3.0, 0.45, 0.35);
        }
        Event::Stopped => tone(&mut out, 0, n(280.0), 620.0, 280.0, 2.5, 0.5, 0.25),
    }
    out
}

/// 16-bit mono PCM WAV of `samples` at `gain`.
fn wav(samples: &[f32], gain: f32) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| (((s * gain).clamp(-1.0, 1.0) * 32_000.0) as i16).to_le_bytes())
        .collect();
    let mut out = Vec::with_capacity(44 + data.len());
    let len = data.len() as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_sounds_are_short_wav_files_at_their_volume() {
        for e in Event::ALL {
            let s = synth(e);
            let secs = s.len() as f32 / RATE as f32;
            assert!(secs > 0.02 && secs < 0.6, "{e:?}: {secs}");
            let loud = wav(&s, 1.0);
            assert_eq!(&loud[..4], b"RIFF");
            assert_eq!(&loud[8..16], b"WAVEfmt ");
            assert_eq!(loud.len(), 44 + 2 * s.len());
            let peak = |w: &[u8]| {
                w[44..]
                    .chunks(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
                    .max()
                    .unwrap()
            };
            let quiet = wav(&s, 0.25);
            assert!(peak(&loud) > 3 * peak(&quiet), "{e:?}");
            assert!(peak(&loud) > 4000, "{e:?} is audible");
        }
        assert_eq!(Event::named("stopped"), Some(Event::Stopped));
        assert_eq!(Event::named("boom"), None);
    }

    #[test]
    fn the_volume_goes_to_the_player_as_it_takes_it() {
        let p = |kind| Program {
            path: "x".into(),
            kind,
        };
        let f = Path::new("/c/click.wav");
        let s = |v: Vec<OsString>| {
            v.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s(p(Kind::PwPlay).args(f, 60)),
            ["--volume=0.60", "/c/click.wav"]
        );
        assert_eq!(
            s(p(Kind::Paplay).args(f, 50)),
            ["--volume=32768", "/c/click.wav"]
        );
        assert_eq!(
            s(p(Kind::Afplay).args(f, 100)),
            ["-v", "1.00", "/c/click.wav"]
        );
        assert_eq!(s(p(Kind::Aplay).args(f, 10)), ["-q", "/c/click.wav"]);
    }

    /// A fake player in a folder of its own: it writes its arguments to
    /// `calls` (one line each time).
    #[cfg(unix)]
    pub(crate) fn fake_player(dir: &Path) -> (OsString, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let calls = dir.join("calls");
        let name = if cfg!(target_os = "macos") {
            "afplay"
        } else {
            "pw-play"
        };
        let script = bin.join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n", calls.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin.into_os_string(), calls)
    }

    #[cfg(unix)]
    fn wait_for_lines(calls: &Path, n: usize) -> Vec<String> {
        for _ in 0..200 {
            let text = std::fs::read_to_string(calls).unwrap_or_default();
            if text.lines().count() >= n {
                return text.lines().map(str::to_string).collect();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::read_to_string(calls)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[cfg(unix)]
    #[test]
    fn sounds_go_to_the_systems_player_rate_limited_and_only_when_on() {
        let dir = std::env::temp_dir().join(format!("cu-sound-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (path, calls) = fake_player(&dir);
        let mut player = Player::with(dir.join("sounds"), Some(path));
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);

        // Off by default: nothing at all.
        let off = OverlayConfig::default();
        for e in Event::ALL {
            assert_eq!(player.play(e, &off, at(0)), None);
        }
        assert!(!dir.join("sounds").exists(), "nothing written while off");

        let on = OverlayConfig {
            sound: true,
            sound_volume: 40,
            sound_key: false,
            ..OverlayConfig::default()
        };
        let click = player.play(Event::Click, &on, at(0)).unwrap();
        assert!(
            click.ends_with("sounds/click-40.wav"),
            "{}",
            click.display()
        );
        assert!(click.is_file());
        // Too soon after: skipped; a sound that is off: skipped.
        assert_eq!(player.play(Event::Click, &on, at(30)), None);
        assert_eq!(player.play(Event::Key, &on, at(40)), None);
        assert!(player.play(Event::Click, &on, at(200)).is_some());
        // An own file, at the volume asked.
        let own = dir.join("ding.wav");
        std::fs::write(&own, wav(&synth(Event::Done), 1.0)).unwrap();
        let mine = OverlayConfig {
            sound_done_file: own.display().to_string(),
            ..on.clone()
        };
        assert_eq!(player.play(Event::Done, &mine, at(300)), Some(own.clone()));
        // (The fake players are done before the next: no more than three
        // play at once.)
        wait_for_lines(&calls, 3);
        std::thread::sleep(Duration::from_millis(100));
        // A file that isn't there: the built-in one, said once.
        let gone = OverlayConfig {
            sound_error_file: dir.join("nope.wav").display().to_string(),
            ..on.clone()
        };
        let e = player.play(Event::Error, &gone, at(400)).unwrap();
        assert!(e.ends_with("error-40.wav"));
        assert_eq!(player.take_warnings().len(), 1);
        let lines = wait_for_lines(&calls, 4);
        let vol = if cfg!(target_os = "macos") {
            "-v 1.00"
        } else {
            "--volume=1.00"
        };
        let own_vol = if cfg!(target_os = "macos") {
            "-v 0.40"
        } else {
            "--volume=0.40"
        };
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(
            lines[0].starts_with(vol) && lines[0].ends_with("click-40.wav"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| *l == format!("{own_vol} {}", own.display())),
            "{lines:?}"
        );

        // No more than a few a second, however many are asked for.
        let mut many = 0;
        for i in 0..40u64 {
            if player.play(Event::Click, &on, at(2000 + i * 81)).is_some() {
                many += 1;
            }
        }
        assert!(many <= 6 * 4, "{many}");

        // No player at all: silent, said once.
        let mut none = Player::with(dir.join("sounds"), Some(OsString::from("/nonexistent")));
        assert_eq!(none.play(Event::Click, &on, at(0)), None);
        assert_eq!(none.play(Event::Done, &on, at(10)), None);
        assert_eq!(none.take_warnings().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
