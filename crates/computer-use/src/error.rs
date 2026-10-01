use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors surfaced to the model as tool errors. Messages are written for the
/// model: they say what went wrong and what to do next.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("unknown tool `{0}`")]
    UnknownTool(String),

    #[error(
        "app `{0}` is not running. Call list_apps to see running apps, or launch_app to start it."
    )]
    AppNotFound(String),

    #[error("`{query}` matches several apps: {candidates}. Use the exact name, id or pid.")]
    AmbiguousApp { query: String, candidates: String },

    #[error("no window matching `{query}` in {app}. Available windows: {available}")]
    WindowNotFound {
        query: String,
        app: String,
        available: String,
    },

    #[error("{app} has no open windows.")]
    NoWindows { app: String },

    #[error(
        "unknown element_index {index} for {app}. Element indices are only valid for the latest get_app_state; call get_app_state again."
    )]
    UnknownElement { app: String, index: u32 },

    #[error(
        "call get_app_state for {0} first; actions need a current accessibility tree and screenshot."
    )]
    NoState(String),

    #[error("{0} is blocked: {1}")]
    Blocked(String, String),

    #[error("the user denied access to {0}. Do not retry; ask the user how to proceed.")]
    Denied(String),

    #[error("{0}")]
    ActionFailed(String),

    /// The action was sent, but the app didn't answer in time: it may or may
    /// not have happened (a busy app, or one that opened a dialog and waits
    /// on it). Never repeated blindly.
    #[error(
        "{0} didn't answer in time: the action was sent but may or may not have happened (the app may be busy or showing a dialog). Look (get_app_state) before doing it again."
    )]
    Unanswered(String),

    #[error(
        "the user stopped the agent with the emergency stop key ({0}). Stop here: don't retry, and ask the user how to proceed. Only the user can let the agent continue (by pressing {0} again)."
    )]
    Stopped(String),

    #[error("cancelled by the client")]
    Cancelled,

    #[error(
        "paused: the user has been using the mouse or keyboard for {0}s, so the action was not run. Try again later, or ask the user."
    )]
    UserBusy(u64),

    #[error("not supported on this platform: {0}")]
    Unsupported(String),

    #[error("missing permission: {0}")]
    Permission(String),

    #[error("platform error: {0}")]
    Platform(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn platform(e: impl std::fmt::Display) -> Self {
        Error::Platform(e.to_string())
    }

    pub fn action(e: impl std::fmt::Display) -> Self {
        Error::ActionFailed(e.to_string())
    }
}
