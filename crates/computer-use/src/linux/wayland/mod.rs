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
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
        || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
}
