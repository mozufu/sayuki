//! Input policy primitives shared by keybindings, IPC, and compositor actions.

/// A user-facing keybinding label such as `Alt+Enter`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeybindingLabel(String);

impl KeybindingLabel {
    /// Stores a normalized keybinding label for later parsing by the compositor.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    /// Returns the label as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
