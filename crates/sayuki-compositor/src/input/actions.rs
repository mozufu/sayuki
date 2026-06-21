use crate::{
    config::BindingActionConfig,
    grabs::ResizeEdge,
    wm::{
        WorkspaceRef,
        swap::{Direction, SwapTarget},
    },
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CompositorAction {
    None,
    Quit,
    Spawn(Vec<String>),
    BeginMove,
    BeginResize(ResizeEdge),
    /// Switch the active canvas to the referenced workspace (index or name).
    SwitchWorkspace(WorkspaceRef),
    /// Move the focused window to the referenced workspace (index or name).
    MoveToWorkspace(WorkspaceRef),
    /// Pan the viewport(s) by a logical-pixel delta.
    PanViewport {
        dx: i32,
        dy: i32,
    },
    /// Zoom the viewport(s) by a multiplicative factor about the center.
    ZoomViewport(f64),
    /// Toggle the fit-all overview on the focused output.
    ToggleOverview,
    /// Toggle the persistent minimap on the focused output.
    ToggleMinimap,
    /// Pin/unpin the focused window to its output's viewport.
    TogglePin,
    /// Swap the focused window with another by direction or MRU order.
    SwapWindow(SwapTarget),
    /// Cycle focus through the active canvas's MRU stack.
    FocusNext,
    FocusPrev,
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
                Ok(Self::SwitchWorkspace(workspace.clone()))
            }
            BindingActionConfig::MoveToWorkspace { workspace } => {
                Ok(Self::MoveToWorkspace(workspace.clone()))
            }
            BindingActionConfig::PanViewport { dx, dy } => {
                Ok(Self::PanViewport { dx: *dx, dy: *dy })
            }
            BindingActionConfig::ZoomViewport { factor } => Ok(Self::ZoomViewport(*factor)),
            BindingActionConfig::ToggleOverview => Ok(Self::ToggleOverview),
            BindingActionConfig::ToggleMinimap => Ok(Self::ToggleMinimap),
            BindingActionConfig::TogglePin => Ok(Self::TogglePin),
            BindingActionConfig::SwapWindow { target } => {
                parse_swap_target(target).map(Self::SwapWindow)
            }
            BindingActionConfig::FocusNext => Ok(Self::FocusNext),
            BindingActionConfig::FocusPrev => Ok(Self::FocusPrev),
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

fn parse_swap_target(target: &str) -> Result<SwapTarget, String> {
    let normalized = target.trim().to_ascii_lowercase();
    let swap = match normalized.as_str() {
        "left" => SwapTarget::Direction(Direction::Left),
        "right" => SwapTarget::Direction(Direction::Right),
        "up" => SwapTarget::Direction(Direction::Up),
        "down" => SwapTarget::Direction(Direction::Down),
        "next" => SwapTarget::Next,
        "prev" | "previous" => SwapTarget::Prev,
        _ => {
            return Err(format!(
                "unknown swap target `{target}`; expected one of left, right, up, down, next, prev"
            ));
        }
    };

    Ok(swap)
}
