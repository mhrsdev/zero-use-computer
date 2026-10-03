//! Wayland sessions: Hyprland, sway and the other wlroots-family
//! compositors. A Wayland app doesn't know where its windows are and other
//! programs can't send it input or read its pixels the X11 way, so:
//!
//! - [`ipc`] asks the compositor (Hyprland's or sway's IPC socket) where
//!   the windows are, which has the keyboard, and to focus, move or resize
//!   them;
//! - [`proto`] speaks the compositor's protocols for the rest: the output
//!   layout, screenshots (wlr-screencopy), pointer and keyboard input
//!   (wlr-virtual-pointer, virtual-keyboard) and the user's idle time
//!   (ext-idle-notify).
//!
//! Apps that run under XWayland keep using the X11 path.

pub(crate) mod ipc;
pub(crate) mod proto;

/// Whether the desktop session is Wayland.
pub fn session() -> bool {
    session_in(
        |k| std::env::var(k).ok(),
        |p| std::path::Path::new(p).exists(),
    )
}

/// Whether the session described by `env` is Wayland: not one that says it
/// is X11, and with a compositor socket that is really there (a variable
/// left over from another session, over ssh or in a container, names a
/// socket that is gone). The socket is `$WAYLAND_DISPLAY` (absolute, or in
/// `$XDG_RUNTIME_DIR`), or `wayland-0` when only the session type says.
fn session_in(env: impl Fn(&str) -> Option<String>, exists: impl Fn(&str) -> bool) -> bool {
    let kind = env("XDG_SESSION_TYPE").unwrap_or_default();
    if kind.eq_ignore_ascii_case("x11") {
        return false;
    }
    let display = match env("WAYLAND_DISPLAY").filter(|d| !d.is_empty()) {
        Some(d) => d,
        None if kind.eq_ignore_ascii_case("wayland") => "wayland-0".into(),
        None => return false,
    };
    if display.starts_with('/') {
        return exists(&display);
    }
    env("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .is_some_and(|dir| exists(&format!("{}/{display}", dir.trim_end_matches('/'))))
}

#[cfg(test)]
mod tests {
    use super::session_in;

    fn check(vars: &[(&str, &str)], sockets: &[&str]) -> bool {
        session_in(
            |k| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            },
            |p| sockets.contains(&p),
        )
    }

    #[test]
    fn a_wayland_session_needs_its_socket() {
        let run = ("XDG_RUNTIME_DIR", "/run/user/1000");
        let wl = ("WAYLAND_DISPLAY", "wayland-1");
        let sock = "/run/user/1000/wayland-1";
        assert!(check(&[run, wl], &[sock]));
        assert!(check(&[run, wl, ("XDG_SESSION_TYPE", "wayland")], &[sock]));
        // A variable left over from another session.
        assert!(!check(&[run, wl], &[]));
        assert!(!check(&[run, wl, ("XDG_SESSION_TYPE", "wayland")], &[]));
        // An X11 session with a nested compositor's variable.
        assert!(!check(&[run, wl, ("XDG_SESSION_TYPE", "x11")], &[sock]));
        // Only the session type says: the default socket.
        assert!(check(
            &[run, ("XDG_SESSION_TYPE", "wayland")],
            &["/run/user/1000/wayland-0"]
        ));
        assert!(!check(&[run, ("XDG_SESSION_TYPE", "tty")], &[sock]));
        // An absolute socket path.
        assert!(check(&[("WAYLAND_DISPLAY", "/tmp/wl")], &["/tmp/wl"]));
        assert!(!check(&[("WAYLAND_DISPLAY", "/tmp/wl")], &[]));
        // No runtime directory: nowhere to look.
        assert!(!check(&[wl], &[sock]));
        assert!(!check(&[], &[]));
    }
}
