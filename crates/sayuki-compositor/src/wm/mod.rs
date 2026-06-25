//! Re-export of the window-management library (`sayuki-wm`) so existing
//! `crate::wm::…` paths keep resolving after the policy + canvas model moved out
//! of the compositor binary. The Smithay-mutation glue that drives this policy
//! lives in `state/actions.rs` and `state/project.rs`.

pub(crate) use sayuki_wm::{
    Canvas, CanvasId, PanCouple, WindowManager, WorkspaceRef, focus, pin, snap, swap, viewport,
};
