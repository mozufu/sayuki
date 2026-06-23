//! Wire types and frame codec for Sayuki's Unix-socket IPC control plane.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct WindowId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub u32);

/// IPC command sent by clients such as `sayukictl`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Request {
    GetVersion,
    GetWindows,
    GetWorkspaces,
    GetOutputs,
    GetFocused,
    Action { action: Action },
}

/// IPC reply sent for one request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    Ok,
    Error {
        message: String,
    },
    Version {
        compositor: String,
        protocol: u32,
    },
    Windows {
        windows: Vec<WindowInfo>,
    },
    Workspaces {
        workspaces: Vec<WorkspaceInfo>,
    },
    Outputs {
        outputs: Vec<OutputInfo>,
    },
    Focused {
        window: Option<WindowId>,
        workspace: WorkspaceId,
    },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct OutputMode {
    pub width: i32,
    pub height: i32,
    pub refresh: i32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct WindowInfo {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub workspace: WorkspaceId,
    // Tiling is deferred; all current toplevels are floating.
    pub floating: bool,
    pub focused: bool,
    pub geometry: Option<Rect>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct WorkspaceInfo {
    pub id: WorkspaceId,
    pub name: String,
    pub project_path: Option<String>,
    pub active: bool,
    pub window_ids: Vec<WindowId>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct OutputInfo {
    pub name: String,
    pub make: String,
    pub model: String,
    pub mode: Option<OutputMode>,
    pub scale: f64,
    pub transform: String,
    pub position: Point,
    pub work_area: Option<Rect>,
}

/// Subscribable compositor event.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    ActionInvoked { action: Action },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Action,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Noop,
    Quit,
    Spawn { argv: Vec<String> },
    BeginMove,
    BeginResize { edges: ResizeEdge },
    SwitchWorkspace { workspace: WorkspaceRef },
    MoveToWorkspace { workspace: WorkspaceRef },
    PanViewport { dx: i32, dy: i32 },
    ZoomViewport { factor: f64 },
    ToggleOverview,
    ToggleMinimap,
    TogglePin,
    SwapWindow { target: SwapTarget },
    FocusNext,
    FocusPrev,
    ToggleHelp,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum WorkspaceRef {
    Index(u8),
    Name(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResizeEdge {
    None,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum SwapTarget {
    Direction { direction: Direction },
    Next,
    Prev,
}

#[derive(Debug)]
pub enum FrameError {
    Json(serde_json::Error),
    Oversized { len: usize, max: usize },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "invalid JSON frame: {error}"),
            Self::Oversized { len, max } => {
                write!(formatter, "IPC frame length {len} exceeds maximum {max}")
            }
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Oversized { .. } => None,
        }
    }
}

pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(value).map_err(FrameError::Json)?;
    if body.len() > MAX_FRAME_LEN {
        return Err(FrameError::Oversized {
            len: body.len(),
            max: MAX_FRAME_LEN,
        });
    }

    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn try_decode_frame<T: DeserializeOwned>(
    buffer: &mut Vec<u8>,
) -> Result<Option<T>, FrameError> {
    if buffer.len() < 4 {
        return Ok(None);
    }

    let len = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if len > MAX_FRAME_LEN {
        return Err(FrameError::Oversized {
            len,
            max: MAX_FRAME_LEN,
        });
    }

    let frame_len = 4 + len;
    if buffer.len() < frame_len {
        return Ok(None);
    }

    let result = serde_json::from_slice(&buffer[4..frame_len]).map_err(FrameError::Json);
    buffer.drain(0..frame_len);
    result.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_codec_round_trips_partial_request() {
        let request = Request::Action {
            action: Action::SwitchWorkspace {
                workspace: WorkspaceRef::Index(2),
            },
        };
        let frame = encode_frame(&request).expect("frame");
        let mut buffer = frame[..2].to_vec();

        assert!(matches!(try_decode_frame::<Request>(&mut buffer), Ok(None)));

        buffer.extend_from_slice(&frame[2..]);
        assert_eq!(
            try_decode_frame::<Request>(&mut buffer).expect("decoded"),
            Some(request)
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buffer = Vec::from((MAX_FRAME_LEN as u32 + 1).to_le_bytes());

        assert!(matches!(
            try_decode_frame::<Request>(&mut buffer),
            Err(FrameError::Oversized { len, max })
                if len == MAX_FRAME_LEN + 1 && max == MAX_FRAME_LEN
        ));
    }

    #[test]
    fn action_json_uses_kebab_case_tags() {
        let value = serde_json::to_value(Action::Spawn {
            argv: vec!["ghostty".to_owned()],
        })
        .expect("json");

        assert_eq!(
            value,
            serde_json::json!({"type": "spawn", "argv": ["ghostty"]})
        );
    }

    #[test]
    fn query_requests_use_kebab_case_tags() {
        assert_eq!(
            serde_json::to_value(Request::GetWindows).expect("json"),
            serde_json::json!({"type": "get-windows"})
        );
        assert_eq!(
            serde_json::to_value(Request::GetWorkspaces).expect("json"),
            serde_json::json!({"type": "get-workspaces"})
        );
        assert_eq!(
            serde_json::to_value(Request::GetOutputs).expect("json"),
            serde_json::json!({"type": "get-outputs"})
        );
        assert_eq!(
            serde_json::to_value(Request::GetFocused).expect("json"),
            serde_json::json!({"type": "get-focused"})
        );
    }

    #[test]
    fn windows_reply_frame_round_trips() {
        let reply = Reply::Windows {
            windows: vec![WindowInfo {
                id: WindowId(7),
                app_id: Some("ghostty".to_owned()),
                title: Some("zsh".to_owned()),
                workspace: WorkspaceId(1),
                floating: true,
                focused: true,
                geometry: Some(Rect {
                    x: 10,
                    y: 20,
                    width: 800,
                    height: 600,
                }),
            }],
        };
        let frame = encode_frame(&reply).expect("frame");
        let mut buffer = frame;

        assert_eq!(
            try_decode_frame::<Reply>(&mut buffer).expect("decoded"),
            Some(reply)
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn focused_reply_round_trips() {
        let reply = Reply::Focused {
            window: Some(WindowId(3)),
            workspace: WorkspaceId(2),
        };
        let frame = encode_frame(&reply).expect("frame");
        let mut buffer = frame;

        assert_eq!(
            try_decode_frame::<Reply>(&mut buffer).expect("decoded"),
            Some(reply)
        );
        assert!(buffer.is_empty());
    }
}
