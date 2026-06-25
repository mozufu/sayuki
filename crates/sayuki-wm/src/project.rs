//! Project session policy (milestone 5b): the data a project canvas carries.
//!
//! A canvas can carry a *project context*: a working directory, a small env
//! overlay, lifecycle hooks, window rules, and a declarative app set. **direnv
//! owns the environment; Sayuki owns the windows/session** — the two compose on
//! the same directory.
//!
//! This module holds the pure policy types — what a project *looks like* and how
//! its rules match. Discovery (`.sayuki` parsing), the central-config merge, and
//! the trust gate live in the compositor, which feeds these types.

use serde::Deserialize;

use crate::tiling::LayoutMode;

/// A hook or app command: either a shell line (`sh -c`) or an explicit argv.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
pub enum HookCmd {
    Shell(String),
    Args(Vec<String>),
}

impl HookCmd {
    /// The argv to execute for this command.
    pub fn to_args(&self) -> Vec<String> {
        match self {
            HookCmd::Shell(command) => vec!["sh".to_owned(), "-c".to_owned(), command.clone()],
            HookCmd::Args(args) => args.clone(),
        }
    }
}

/// Lifecycle hooks for a project canvas. `on_init` runs once (first activation);
/// `on_enter`/`on_leave` run on every activation/deactivation and must be
/// idempotent. `on_destroy` is reserved for canvas teardown (unused in 5b).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub struct CanvasHooks {
    pub on_init: Option<HookCmd>,
    pub on_enter: Option<HookCmd>,
    pub on_leave: Option<HookCmd>,
    pub on_destroy: Option<HookCmd>,
}

/// A map-time routing rule: a window matching `app_id`/`title` is routed to the
/// project canvas that owns the rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct WindowRule {
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// Route the matching surface back to this canvas.
    #[serde(default)]
    pub pin: bool,
    /// Force the matching window tiled (`Some(true)`) or floating
    /// (`Some(false)`), overriding the canvas layout mode; `None` inherits it.
    #[serde(default)]
    pub tiling: Option<bool>,
}

impl WindowRule {
    /// Whether this rule matches a window's `app_id`/`title`. Each specified
    /// field is a substring test; an unspecified field is a wildcard. A rule
    /// with no fields matches nothing (it would otherwise capture everything).
    pub fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
        if self.app_id.is_none() && self.title.is_none() {
            return false;
        }
        field_matches(self.app_id.as_deref(), app_id) && field_matches(self.title.as_deref(), title)
    }
}

fn field_matches(want: Option<&str>, value: Option<&str>) -> bool {
    match want {
        None => true,
        Some(want) => value.is_some_and(|value| value.contains(want)),
    }
}

/// A canvas's resolved project context: the merge of the central config with the
/// `.sayuki` file (the latter only when trusted). Built by the compositor's
/// `resolve_context`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectContext {
    pub working_dir: Option<std::path::PathBuf>,
    pub env: Vec<(String, String)>,
    pub hooks: CanvasHooks,
    pub rules: Vec<WindowRule>,
    pub apps: Vec<String>,
    /// The canvas's initial layout mode (project `layout = "floating" |
    /// "tiling"`). Floating by default.
    pub layout: LayoutMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_cmd_shell_wraps_in_sh() {
        assert_eq!(
            HookCmd::Shell("firefox -P sayuki".to_owned()).to_args(),
            ["sh", "-c", "firefox -P sayuki"]
        );
        assert_eq!(
            HookCmd::Args(vec!["zed".to_owned(), ".".to_owned()]).to_args(),
            ["zed", "."]
        );
    }

    #[test]
    fn window_rule_matches_app_id_substring() {
        let rule = WindowRule {
            app_id: Some("firefox".to_owned()),
            title: None,
            pin: true,
            tiling: None,
        };
        assert!(rule.matches(Some("firefox"), None));
        assert!(rule.matches(Some("org.mozilla.firefox"), Some("anything")));
        assert!(!rule.matches(Some("ghostty"), None));
        assert!(!rule.matches(None, None));
    }

    #[test]
    fn window_rule_requires_both_specified_fields() {
        let rule = WindowRule {
            app_id: Some("firefox".to_owned()),
            title: Some("sayuki".to_owned()),
            pin: true,
            tiling: None,
        };
        assert!(rule.matches(Some("firefox"), Some("sayuki — Mozilla")));
        // app_id matches but title does not.
        assert!(!rule.matches(Some("firefox"), Some("other")));
    }

    #[test]
    fn empty_rule_matches_nothing() {
        let rule = WindowRule {
            app_id: None,
            title: None,
            pin: true,
            tiling: None,
        };
        assert!(!rule.matches(Some("firefox"), Some("anything")));
    }
}
