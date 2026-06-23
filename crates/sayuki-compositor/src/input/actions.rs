use crate::config::BindingActionConfig;

pub(crate) fn action_from_config(
    config: &BindingActionConfig,
) -> Result<sayuki_ipc::Action, String> {
    match config {
        BindingActionConfig::Quit => Ok(sayuki_ipc::Action::Quit),
        BindingActionConfig::Spawn { command } => Ok(sayuki_ipc::Action::Spawn {
            argv: command.clone(),
        }),
        BindingActionConfig::BeginMove => Ok(sayuki_ipc::Action::BeginMove),
        BindingActionConfig::BeginResize { edges } => Ok(sayuki_ipc::Action::BeginResize {
            edges: parse_resize_edges(edges)?,
        }),
        BindingActionConfig::SwitchWorkspace { workspace } => {
            Ok(sayuki_ipc::Action::SwitchWorkspace {
                workspace: ipc_workspace_ref(workspace.clone()),
            })
        }
        BindingActionConfig::MoveToWorkspace { workspace } => {
            Ok(sayuki_ipc::Action::MoveToWorkspace {
                workspace: ipc_workspace_ref(workspace.clone()),
            })
        }
        BindingActionConfig::PanViewport { dx, dy } => {
            Ok(sayuki_ipc::Action::PanViewport { dx: *dx, dy: *dy })
        }
        BindingActionConfig::ZoomViewport { factor } => {
            Ok(sayuki_ipc::Action::ZoomViewport { factor: *factor })
        }
        BindingActionConfig::ToggleOverview => Ok(sayuki_ipc::Action::ToggleOverview),
        BindingActionConfig::ToggleMinimap => Ok(sayuki_ipc::Action::ToggleMinimap),
        BindingActionConfig::TogglePin => Ok(sayuki_ipc::Action::TogglePin),
        BindingActionConfig::SwapWindow { target } => Ok(sayuki_ipc::Action::SwapWindow {
            target: parse_swap_target(target)?,
        }),
        BindingActionConfig::FocusNext => Ok(sayuki_ipc::Action::FocusNext),
        BindingActionConfig::FocusPrev => Ok(sayuki_ipc::Action::FocusPrev),
        BindingActionConfig::ToggleHelp => Ok(sayuki_ipc::Action::ToggleHelp),
    }
}

pub(crate) fn action_label(action: &sayuki_ipc::Action) -> &'static str {
    match action {
        sayuki_ipc::Action::Noop => "No action",
        sayuki_ipc::Action::Quit => "Quit Sayuki",
        sayuki_ipc::Action::Spawn { .. } => "Spawn command",
        sayuki_ipc::Action::BeginMove => "Move focused window",
        sayuki_ipc::Action::BeginResize { .. } => "Resize focused window",
        sayuki_ipc::Action::SwitchWorkspace { .. } => "Switch workspace",
        sayuki_ipc::Action::MoveToWorkspace { .. } => "Move window to workspace",
        sayuki_ipc::Action::PanViewport { .. } => "Pan viewport",
        sayuki_ipc::Action::ZoomViewport { factor } if *factor > 1.0 => "Zoom in",
        sayuki_ipc::Action::ZoomViewport { .. } => "Zoom out",
        sayuki_ipc::Action::ToggleOverview => "Toggle overview",
        sayuki_ipc::Action::ToggleMinimap => "Toggle minimap",
        sayuki_ipc::Action::TogglePin => "Pin or unpin window",
        sayuki_ipc::Action::SwapWindow { .. } => "Swap focused window",
        sayuki_ipc::Action::FocusNext => "Focus next window",
        sayuki_ipc::Action::FocusPrev => "Focus previous window",
        sayuki_ipc::Action::ToggleHelp => "Toggle keymap help",
    }
}

fn parse_resize_edges(edges: &str) -> Result<sayuki_ipc::ResizeEdge, String> {
    let normalized = edges.trim().to_ascii_lowercase().replace('_', "-");
    let edge = match normalized.as_str() {
        "none" => sayuki_ipc::ResizeEdge::None,
        "top" => sayuki_ipc::ResizeEdge::Top,
        "bottom" => sayuki_ipc::ResizeEdge::Bottom,
        "left" => sayuki_ipc::ResizeEdge::Left,
        "right" => sayuki_ipc::ResizeEdge::Right,
        "top-left" | "left-top" => sayuki_ipc::ResizeEdge::TopLeft,
        "top-right" | "right-top" => sayuki_ipc::ResizeEdge::TopRight,
        "bottom-left" | "left-bottom" => sayuki_ipc::ResizeEdge::BottomLeft,
        "bottom-right" | "right-bottom" => sayuki_ipc::ResizeEdge::BottomRight,
        _ => {
            return Err(format!(
                "unknown resize edge `{edges}`; expected one of none, top, bottom, left, right, top-left, top-right, bottom-left, bottom-right"
            ));
        }
    };

    Ok(edge)
}

fn parse_swap_target(target: &str) -> Result<sayuki_ipc::SwapTarget, String> {
    let normalized = target.trim().to_ascii_lowercase();
    let swap = match normalized.as_str() {
        "left" => sayuki_ipc::SwapTarget::Direction {
            direction: sayuki_ipc::Direction::Left,
        },
        "right" => sayuki_ipc::SwapTarget::Direction {
            direction: sayuki_ipc::Direction::Right,
        },
        "up" => sayuki_ipc::SwapTarget::Direction {
            direction: sayuki_ipc::Direction::Up,
        },
        "down" => sayuki_ipc::SwapTarget::Direction {
            direction: sayuki_ipc::Direction::Down,
        },
        "next" => sayuki_ipc::SwapTarget::Next,
        "prev" | "previous" => sayuki_ipc::SwapTarget::Prev,
        _ => {
            return Err(format!(
                "unknown swap target `{target}`; expected one of left, right, up, down, next, prev"
            ));
        }
    };

    Ok(swap)
}

fn ipc_workspace_ref(reference: crate::wm::WorkspaceRef) -> sayuki_ipc::WorkspaceRef {
    match reference {
        crate::wm::WorkspaceRef::Index(index) => sayuki_ipc::WorkspaceRef::Index(index),
        crate::wm::WorkspaceRef::Name(name) => sayuki_ipc::WorkspaceRef::Name(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_workspace_maps_to_ipc_action() {
        let config = BindingActionConfig::SwitchWorkspace {
            workspace: crate::wm::WorkspaceRef::Index(2),
        };

        assert_eq!(
            action_from_config(&config).expect("action"),
            sayuki_ipc::Action::SwitchWorkspace {
                workspace: sayuki_ipc::WorkspaceRef::Index(2)
            }
        );
    }

    #[test]
    fn resize_and_swap_parse_to_ipc_types() {
        let resize = BindingActionConfig::BeginResize {
            edges: "bottom-right".to_owned(),
        };
        assert_eq!(
            action_from_config(&resize).expect("resize action"),
            sayuki_ipc::Action::BeginResize {
                edges: sayuki_ipc::ResizeEdge::BottomRight
            }
        );

        let swap = BindingActionConfig::SwapWindow {
            target: "previous".to_owned(),
        };
        assert_eq!(
            action_from_config(&swap).expect("swap action"),
            sayuki_ipc::Action::SwapWindow {
                target: sayuki_ipc::SwapTarget::Prev
            }
        );
    }
}
