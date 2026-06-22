//! Configuration data-model scaffolding for Sayuki.

use serde::{Deserialize, Serialize};

/// Source layer for a loaded configuration value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ConfigLayer {
    /// Built-in defaults compiled into Sayuki.
    Defaults,
    /// System-wide configuration.
    System,
    /// Per-user configuration.
    User,
    /// Trusted project-local `.sayuki` configuration.
    Project,
}

/// A value annotated with the layer that supplied it.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LayeredValue<T> {
    /// Configuration layer that supplied `value`.
    pub layer: ConfigLayer,
    /// Layer value.
    pub value: T,
}

impl<T> LayeredValue<T> {
    /// Creates a layer-tagged configuration value.
    #[must_use]
    pub const fn new(layer: ConfigLayer, value: T) -> Self {
        Self { layer, value }
    }
}
