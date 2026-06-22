//! Wire types for Sayuki's Unix-socket IPC control plane.

use serde::{Deserialize, Serialize};

/// IPC command sent by clients such as `sayukictl`.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    /// Health check for the compositor IPC server.
    Ping,
    /// Ask the compositor to execute a named action.
    RunAction { action: String },
}

/// IPC reply sent for one request.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Response {
    /// Successful command completion.
    Ok,
    /// Command failure with a human-readable reason.
    Error { message: String },
}

/// Subscribable compositor event.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    /// The active project/canvas changed.
    WorkspaceChanged { name: String },
    /// A compositor action was accepted.
    ActionInvoked { action: String },
}
