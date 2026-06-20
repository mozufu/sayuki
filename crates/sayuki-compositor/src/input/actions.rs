use crate::{config::BindingActionConfig, grabs::ResizeEdge};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompositorAction {
    None,
    Quit,
    Spawn(Vec<String>),
    BeginMove,
    BeginResize(ResizeEdge),
    SwitchWorkspace(u8),
}

impl CompositorAction {
    pub(crate) fn from_config(config: &BindingActionConfig) -> Result<Self, String> {
        match config {
            BindingActionConfig::Quit => Ok(Self::Quit),
            BindingActionConfig::Spawn { command } => Ok(Self::Spawn(command.clone())),
            BindingActionConfig::BeginMove => Ok(Self::BeginMove),
            BindingActionConfig::BeginResize { edges } => {
                parse_resize_edges(edges).map(Self::BeginResize)
            }
            BindingActionConfig::SwitchWorkspace { workspace } => {
                Ok(Self::SwitchWorkspace(*workspace))
            }
        }
    }
}

fn parse_resize_edges(edges: &str) -> Result<ResizeEdge, String> {
    let normalized = edges.trim().to_ascii_lowercase().replace('_', "-");
    let edge = match normalized.as_str() {
        "none" => ResizeEdge::NONE,
        "top" => ResizeEdge::TOP,
        "bottom" => ResizeEdge::BOTTOM,
        "left" => ResizeEdge::LEFT,
        "right" => ResizeEdge::RIGHT,
        "top-left" | "left-top" => ResizeEdge::TOP_LEFT,
        "top-right" | "right-top" => ResizeEdge::TOP_RIGHT,
        "bottom-left" | "left-bottom" => ResizeEdge::BOTTOM_LEFT,
        "bottom-right" | "right-bottom" => ResizeEdge::BOTTOM_RIGHT,
        _ => {
            return Err(format!(
                "unknown resize edge `{edges}`; expected one of none, top, bottom, left, right, top-left, top-right, bottom-left, bottom-right"
            ));
        }
    };

    Ok(edge)
}
