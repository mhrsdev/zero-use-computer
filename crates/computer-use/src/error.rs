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
