//! Core compositor runtime primitives shared across Sayuki crates.

/// Stable compositor application id used for sockets, logs, and protocol labels.
pub const APP_ID: &str = "sayuki";

/// Static metadata for a Sayuki runtime component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentInfo {
    /// Machine-readable component name.
    pub name: &'static str,
    /// Human-readable component purpose.
    pub purpose: &'static str,
}

impl ComponentInfo {
    /// Creates static component metadata.
    #[must_use]
    pub const fn new(name: &'static str, purpose: &'static str) -> Self {
        Self { name, purpose }
    }
}
