//! Window-management policy types for Sayuki's project canvas model.

/// Identifier for a project canvas in the window-manager model.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanvasId(pub u32);

/// Reference to a workspace/project canvas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRef {
    /// Select by zero-based index in the configured project list.
    Index(usize),
    /// Select by project/canvas name.
    Name(String),
}
