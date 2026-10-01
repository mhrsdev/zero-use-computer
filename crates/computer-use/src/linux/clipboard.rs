//! Linux clipboard via whichever helper is installed: `wl-copy`/`wl-paste`
//! (Wayland), `xclip`, or `xsel`. This avoids owning an X11 selection (which
//! would require a long-lived event loop just to serve a paste).

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// (get-command, get-args, set-command, set-args) for each supported helper.
const HELPERS: &[(&str, &[&str], &str, &[&str])] = &[
    ("wl-paste", &["--no-newline"], "wl-copy", &[]),
    (
        "xclip",
        &["-selection", "clipboard", "-o"],
        "xclip",
        &["-selection", "clipboard", "-i"],
    ),
    (
        "xsel",
        &["--clipboard", "--output"],
        "xsel",
        &["--clipboard", "--input"],
    ),
];

fn is_enoent(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
}

pub fn get() -> Result<String> {
    let mut missing = true;
    for (cmd, args, _, _) in HELPERS {
        // Never inherit our stdin: it carries the MCP JSON-RPC stream.
        match Command::new(cmd).args(*args).stdin(Stdio::null()).output() {
            Ok(out) if out.status.success() => {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
            Ok(_) => missing = false,
            Err(ref e) if is_enoent(e) => continue,
            Err(e) => return Err(Error::Platform(format!("{cmd}: {e}"))),
        }
    }
    Err(unavailable(missing))
}

pub fn set(text: &str) -> Result<()> {
    let mut missing = true;
    for (_, _, cmd, args) in HELPERS {
        // wl-copy/xclip keep running to serve the selection: they must not
        // hold on to our stdout (the MCP JSON-RPC stream) or stderr; xclip
        // forks a daemon that would otherwise hold it open after we exit.
        let spawned = Command::new(cmd)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(ref e) if is_enoent(e) => continue,
            Err(e) => return Err(Error::Platform(format!("{cmd}: {e}"))),
        };
        missing = false;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| Error::Platform(format!("{cmd}: {e}")))?;
        }
        // Close stdin, then wait (wl-copy forks a daemon and returns promptly).
        drop(child.stdin.take());
        let status = child
            .wait()
            .map_err(|e| Error::Platform(format!("{cmd}: {e}")))?;
        if status.success() {
            return Ok(());
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
