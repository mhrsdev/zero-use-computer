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
        // Text only: an image on the clipboard isn't text.
        get: ("wl-paste", &["--no-newline", "--type", "text"]),
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
    let wayland = super::wayland::session();
    HELPERS.iter().filter(move |h| wayland || !h.wayland)
}

/// Whether a helper that failed (with nothing on stdout) said that the
/// clipboard has no text: it is empty, or holds something else (an image).
fn says_empty(stdout: &[u8], stderr: &str) -> bool {
    let e = stderr.to_ascii_lowercase();
    stdout.is_empty()
        && (e.contains("nothing is copied")
            || e.contains("no selection")
            || e.contains("no suitable type")
            || (e.contains("target") && e.contains("not available")))
}

/// Read all of a pipe on another thread, so that a helper that never
/// finishes (the clipboard's owner doesn't answer) can be given up on.
fn read_all(pipe: Option<impl Read + Send + 'static>) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

/// Write `data` to a child's stdin and wait for it to exit, all within
/// `deadline` (a helper that doesn't read its input would block the write).
/// Whether the write went through (or the error), and how it exited
/// (`None`: killed).
fn feed(child: &mut Child, data: &[u8], deadline: Instant) -> (std::io::Result<()>, Option<bool>) {
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut stdin) = child.stdin.take() {
        let data = data.to_vec();
        std::thread::spawn(move || {
            // Stdin is closed when this ends.
            let _ = tx.send(stdin.write_all(&data));
        });
    } else {
        let _ = tx.send(Ok(()));
    }
    let written = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(r) => r,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "it didn't take the text in time",
        )),
    };
    // Killed if it's still running: the writer's pipe then breaks too.
    let done = wait_until(child, deadline);
    (written, done)
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
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(ref e) if is_enoent(e) => continue,
            Err(e) => return Err(Error::Platform(format!("{cmd}: {e}"))),
        };
        missing = false;
        let deadline = Instant::now() + DEADLINE;
        let out = read_all(child.stdout.take());
        let err = read_all(child.stderr.take());
        let read = out.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let status = wait_until(&mut child, deadline);
        let stderr = err
            .recv_timeout(Duration::from_millis(100))
            .map(|e| String::from_utf8_lossy(&e).into_owned())
            .unwrap_or_default();
        match (status, read) {
            (Some(true), Ok(out)) => return Ok(String::from_utf8_lossy(&out).into_owned()),
            // No text on the clipboard: empty, not a failure (and the next
            // helper, the X11 one in a Wayland session, would read another
            // clipboard).
            (Some(false), Ok(out)) if says_empty(&out, &stderr) => return Ok(String::new()),
            (None, _) | (_, Err(_)) => {
                log::warn!("{cmd} didn't finish within {} s", DEADLINE.as_secs());
                timed_out = Some(cmd);
            }
            _ => log::debug!("{cmd} failed: {}", stderr.trim()),
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
        // Then wait (the helper forks a daemon to serve the clipboard and
        // returns promptly once its input is closed), and reap it whatever
        // happened.
        let (written, done) = feed(&mut child, text.as_bytes(), Instant::now() + DEADLINE);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clipboard_without_text_is_empty_not_an_error() {
        assert!(says_empty(b"", "Nothing is copied\n"));
        assert!(says_empty(b"", "No suitable type of content copied\n"));
        assert!(says_empty(b"", "No selection\n"));
        assert!(says_empty(b"", "Error: target STRING not available\n"));
        assert!(says_empty(b"", "Error: target UTF8_STRING not available\n"));
        // Other failures stay failures.
        assert!(!says_empty(b"", "Failed to connect to a Wayland server\n"));
        assert!(!says_empty(b"", "Error: Can't open display: :0\n"));
        assert!(!says_empty(b"", ""));
        assert!(!says_empty(b"text", "Nothing is copied"));
    }

    #[test]
    fn a_helper_that_doesnt_read_its_input_is_given_up_on() {
        // More than a pipe holds, to a program that never reads it.
        let mut child = Command::new("sleep")
            .arg("10")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("sleep");
        let t = Instant::now();
        let (written, done) = feed(
            &mut child,
            &vec![b'x'; 1 << 20],
            Instant::now() + Duration::from_millis(300),
        );
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
        assert!(written.is_err());
        assert_eq!(done, None, "killed");
    }

    #[test]
    fn a_helper_gets_all_of_the_text() {
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cat");
        let out = read_all(child.stdout.take());
        let (written, done) = feed(&mut child, b"hello", Instant::now() + DEADLINE);
        assert!(written.is_ok());
        assert_eq!(done, Some(true));
        assert_eq!(out.recv_timeout(DEADLINE).unwrap(), b"hello");
    }
}
