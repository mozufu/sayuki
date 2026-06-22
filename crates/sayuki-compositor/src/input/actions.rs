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
    ToggleHelp,
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
            BindingActionConfig::ToggleHelp => Ok(Self::ToggleHelp),
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::None => "No action",
            Self::Quit => "Quit Sayuki",
            Self::Spawn(_) => "Spawn command",
            Self::BeginMove => "Move focused window",
            Self::BeginResize(_) => "Resize focused window",
            Self::SwitchWorkspace(_) => "Switch workspace",
            Self::MoveToWorkspace(_) => "Move window to workspace",
            Self::PanViewport { .. } => "Pan viewport",
            Self::ZoomViewport(factor) if *factor > 1.0 => "Zoom in",
            Self::ZoomViewport(_) => "Zoom out",
            Self::ToggleOverview => "Toggle overview",
            Self::ToggleMinimap => "Toggle minimap",
            Self::TogglePin => "Pin or unpin window",
            Self::SwapWindow(_) => "Swap focused window",
            Self::FocusNext => "Focus next window",
            Self::FocusPrev => "Focus previous window",
            Self::ToggleHelp => "Toggle keymap help",
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
