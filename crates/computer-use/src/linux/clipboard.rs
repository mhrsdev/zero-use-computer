//! Linux clipboard via whichever helper is installed: `wl-copy`/`wl-paste`
//! (Wayland), `xclip`, or `xsel`. This avoids owning an X11 selection (which
//! would require a long-lived event loop just to serve a paste).

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// How long a helper may take: reading the clipboard asks the app that owns
/// it, which may be busy or frozen.
const DEADLINE: Duration = Duration::from_secs(3);

/// A clipboard helper program.
struct Helper {
    get: (&'static str, &'static [&'static str]),
    set: (&'static str, &'static [&'static str]),
    /// Only works in a Wayland session.
    wayland: bool,
}

const HELPERS: &[Helper] = &[
    Helper {
        get: ("wl-paste", &["--no-newline"]),
        set: ("wl-copy", &[]),
        wayland: true,
    },
    Helper {
        get: ("xclip", &["-selection", "clipboard", "-o"]),
        set: ("xclip", &["-selection", "clipboard", "-i"]),
        wayland: false,
    },
    Helper {
        // -t: xsel's own limit on waiting for the clipboard's owner (ms).
        get: ("xsel", &["--clipboard", "--output", "-t", "3000"]),
        set: ("xsel", &["--clipboard", "--input"]),
        wayland: false,
    },
];

/// The helpers that can work here: wl-clipboard only in a Wayland session.
fn helpers() -> impl Iterator<Item = &'static Helper> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty());
    HELPERS.iter().filter(move |h| wayland || !h.wayland)
}

fn is_enoent(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
}

/// Wait for a child until `deadline`; kill it if it isn't done by then.
/// Whether it exited successfully (`None`: killed).
fn wait_until(child: &mut Child, deadline: Instant) -> Option<bool> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

pub fn get() -> Result<String> {
    let mut missing = true;
    let mut timed_out = None;
    for h in helpers() {
        let (cmd, args) = h.get;
        let mut child = match Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(ref e) if is_enoent(e) => continue,
            Err(e) => return Err(Error::Platform(format!("{cmd}: {e}"))),
        };
        missing = false;
        let deadline = Instant::now() + DEADLINE;
        // Read on another thread, so a helper that never finishes (the
        // clipboard's owner doesn't answer) can be given up on.
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(mut out) = child.stdout.take() {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = tx.send(out.read_to_end(&mut buf).map(|_| buf));
            });
        }
        let read = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        match (wait_until(&mut child, deadline), read) {
            (Some(true), Ok(Ok(out))) => return Ok(String::from_utf8_lossy(&out).into_owned()),
            (None, _) | (_, Err(_)) => {
                log::warn!("{cmd} didn't finish within {} s", DEADLINE.as_secs());
                timed_out = Some(cmd);
            }
            _ => {}
        }
    }
    if let Some(cmd) = timed_out {
        return Err(Error::Platform(format!(
            "reading the clipboard timed out ({cmd} got no answer from the app that owns it within {} s)",
            DEADLINE.as_secs()
        )));
    }
    Err(unavailable(missing))
}

pub fn set(text: &str) -> Result<()> {
    let mut missing = true;
    for h in helpers() {
        let (cmd, args) = h.set;
        // Its stdout/stderr must not reach ours (the MCP stdio channel); xclip
        // forks a daemon that would otherwise hold it open after we exit.
        let mut child = match Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(ref e) if is_enoent(e) => continue,
            Err(e) => return Err(Error::Platform(format!("{cmd}: {e}"))),
        };
        missing = false;
        let written = child
            .stdin
            .take()
            .map(|mut stdin| stdin.write_all(text.as_bytes()))
            .unwrap_or(Ok(()));
        // Stdin is closed now; wait (the helper forks a daemon to serve the
        // clipboard and returns promptly), and reap it whatever happened.
        let done = wait_until(&mut child, Instant::now() + DEADLINE);
        match (written, done) {
            (Ok(()), Some(true)) => return Ok(()),
            (Err(e), _) => log::warn!("{cmd}: {e}"),
            (_, None) => log::warn!("{cmd} didn't finish within {} s", DEADLINE.as_secs()),
            _ => {}
        }
    }
    Err(unavailable(missing))
}

fn unavailable(missing: bool) -> Error {
    if missing {
        Error::Unsupported(
            "no clipboard helper found. Install wl-clipboard (Wayland), xclip, or xsel.".into(),
        )
    } else {
        Error::Platform("the clipboard helper failed".into())
    }
}
