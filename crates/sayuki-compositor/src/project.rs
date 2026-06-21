//! Project session layer (milestone 5b policy).
//!
//! A canvas can carry a *project context*: a working directory, a small env
//! overlay, lifecycle hooks, window rules, and a declarative app set. **direnv
//! owns the environment; Sayuki owns the windows/session** — the two compose on
//! the same directory.
//!
//! Two sources feed a project: the user's central `[[project]]` config (parsed
//! in `config.rs`, inherently trusted) and a per-directory `.sayuki` file
//! (discovered here, honored only when the trust gate allows it). Everything in
//! this module is pure and unit-tested; the live spawning/hook execution lives
//! in `state.rs`/`input/spawn.rs`.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// A hook or app command: either a shell line (`sh -c`) or an explicit argv.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
pub(crate) enum HookCmd {
    Shell(String),
    Args(Vec<String>),
}

impl HookCmd {
    /// The argv to execute for this command.
    pub(crate) fn to_args(&self) -> Vec<String> {
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
pub(crate) struct CanvasHooks {
    pub(crate) on_init: Option<HookCmd>,
    pub(crate) on_enter: Option<HookCmd>,
    pub(crate) on_leave: Option<HookCmd>,
    pub(crate) on_destroy: Option<HookCmd>,
}

/// A map-time routing rule: a window matching `app_id`/`title` is routed to the
/// project canvas that owns the rule.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct WindowRule {
    #[serde(default)]
    pub(crate) app_id: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// Route the matching surface back to this canvas.
    #[serde(default)]
    pub(crate) pin: bool,
}

impl WindowRule {
    /// Whether this rule matches a window's `app_id`/`title`. Each specified
    /// field is a substring test; an unspecified field is a wildcard. A rule
    /// with no fields matches nothing (it would otherwise capture everything).
    pub(crate) fn matches(&self, app_id: Option<&str>, title: Option<&str>) -> bool {
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

/// A central `[[project]]` entry (built in `config.rs`). Inherently trusted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectConfig {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) on_init: Option<HookCmd>,
}

/// A discovered `<dir>/.sayuki` file: what the project *looks like*.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default)]
pub(crate) struct SayukiProject {
    /// Layout hint (only `"floating"` is honored in 5b; tiling is deferred).
    pub(crate) layout: Option<String>,
    /// Declarative apps, launched through the direnv-wrapped spawn.
    pub(crate) apps: Vec<String>,
    /// One-shot imperative escape for single-instance apps.
    pub(crate) on_init: Option<HookCmd>,
    #[serde(rename = "window_rule")]
    pub(crate) window_rules: Vec<WindowRule>,
}

impl SayukiProject {
    pub(crate) fn parse(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Read `<dir>/.sayuki`, returning its path and raw content if present.
    pub(crate) fn discover(dir: &Path) -> Option<(PathBuf, String)> {
        let path = dir.join(".sayuki");
        let content = fs::read_to_string(&path).ok()?;
        Some((path, content))
    }
}

/// A canvas's resolved project context: the merge of the central config with the
/// `.sayuki` file (the latter only when trusted).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectContext {
    pub(crate) working_dir: Option<PathBuf>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) hooks: CanvasHooks,
    pub(crate) rules: Vec<WindowRule>,
    pub(crate) apps: Vec<String>,
}

impl ProjectContext {
    /// Merge a central config entry with a `.sayuki` file. Pass `sayuki = Some`
    /// **only when the file is trusted**; an untrusted (or absent) `.sayuki`
    /// contributes no apps, rules, or hooks, so the project still opens with
    /// central-config defaults. The `.sayuki`'s `on_init` takes precedence over
    /// the central one when both are present.
    pub(crate) fn resolve(central: Option<ProjectConfig>, sayuki: Option<SayukiProject>) -> Self {
        let central_on_init = central.as_ref().and_then(|config| config.on_init.clone());
        let (sayuki_on_init, apps, rules) = match sayuki {
            Some(sayuki) => (sayuki.on_init, sayuki.apps, sayuki.window_rules),
            None => (None, Vec::new(), Vec::new()),
        };

        Self {
            working_dir: central.as_ref().map(|config| config.path.clone()),
            env: central.map(|config| config.env).unwrap_or_default(),
            hooks: CanvasHooks {
                on_init: sayuki_on_init.or(central_on_init),
                ..CanvasHooks::default()
            },
            rules,
            apps,
        }
    }
}

/// The trust allowlist, mirroring `direnv allow`: a project's `.sayuki` is
/// honored only when its path is listed **and** the listed content hash still
/// matches (editing the file re-blocks it until re-allowed).
#[derive(Clone, Debug, Default)]
pub(crate) struct TrustStore {
    entries: HashMap<PathBuf, String>,
}

impl TrustStore {
    /// Parse an allowlist: one `<hash> <path>` per line; `#` comments and blank
    /// lines ignored.
    pub(crate) fn parse(content: &str) -> Self {
        let mut entries = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((hash, path)) = line.split_once(char::is_whitespace) {
                entries.insert(PathBuf::from(path.trim()), hash.trim().to_owned());
            }
        }
        Self { entries }
    }

    /// Load the allowlist from `$XDG_STATE_HOME/sayuki/trusted` (falling back to
    /// `~/.local/state/...`). A missing file is an empty (deny-all) store.
    pub(crate) fn load() -> Self {
        let Some(path) = trusted_path() else {
            return Self::default();
        };
        match fs::read_to_string(&path) {
            Ok(content) => Self::parse(&content),
            Err(_) => Self::default(),
        }
    }

    /// Whether `path` is allowed for the current `content` of its `.sayuki`.
    pub(crate) fn is_trusted(&self, path: &Path, content: &str) -> bool {
        self.entries
            .get(path)
            .is_some_and(|hash| hash == &content_hash(content))
    }
}

/// Content hash used by the trust gate to detect edits. This is an edit-detection
/// digest (like direnv's), not a cryptographic authenticator: the threat is "did
/// this allowed file change", and writing a project's `.sayuki` already implies
/// write access to its `.envrc`.
///
/// FNV-1a (64-bit) is used rather than `std`'s `DefaultHasher` because this digest
/// is persisted to the on-disk allowlist: `DefaultHasher`'s output is explicitly
/// not guaranteed stable across toolchains, so an upgrade would silently
/// invalidate every trust entry. FNV-1a is fixed and deterministic.
pub(crate) fn content_hash(content: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn trusted_path() -> Option<PathBuf> {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(state).join("sayuki/trusted"));
    }
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(home).join(".local/state/sayuki/trusted"))
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
        };
        assert!(!rule.matches(Some("firefox"), Some("anything")));
    }

    #[test]
    fn sayuki_project_parses_apps_rules_and_on_init() {
        let project = SayukiProject::parse(
            r#"
layout = "floating"
apps = ["ghostty", "zed ."]
on_init = "firefox -P sayuki --new-window"

[[window_rule]]
app_id = "firefox"
title = "sayuki"
pin = true
"#,
        )
        .expect("valid .sayuki");
        assert_eq!(project.apps, ["ghostty", "zed ."]);
        assert_eq!(
            project.on_init,
            Some(HookCmd::Shell("firefox -P sayuki --new-window".to_owned()))
        );
        assert_eq!(project.window_rules.len(), 1);
        assert!(project.window_rules[0].pin);
        assert_eq!(project.window_rules[0].app_id.as_deref(), Some("firefox"));
    }

    fn config() -> ProjectConfig {
        ProjectConfig {
            name: "sayuki".to_owned(),
            path: PathBuf::from("/p"),
            env: vec![("RUST_LOG".to_owned(), "debug".to_owned())],
            on_init: None,
        }
    }

    #[test]
    fn untrusted_sayuki_contributes_nothing() {
        let sayuki = SayukiProject {
            apps: vec!["ghostty".to_owned()],
            on_init: Some(HookCmd::Shell("firefox".to_owned())),
            ..SayukiProject::default()
        };
        // Untrusted: caller passes `None`, so apps/on_init are dropped.
        let untrusted = ProjectContext::resolve(Some(config()), None);
        assert!(untrusted.apps.is_empty());
        assert_eq!(untrusted.hooks.on_init, None);
        assert_eq!(untrusted.working_dir, Some(PathBuf::from("/p")));

        // Trusted: apps and on_init come through.
        let trusted = ProjectContext::resolve(Some(config()), Some(sayuki));
        assert_eq!(trusted.apps, ["ghostty"]);
        assert_eq!(
            trusted.hooks.on_init,
            Some(HookCmd::Shell("firefox".to_owned()))
        );
    }

    #[test]
    fn central_on_init_is_fallback_for_sayuki() {
        let central = ProjectConfig {
            on_init: Some(HookCmd::Shell("central".to_owned())),
            ..config()
        };
        // No .sayuki: central on_init used.
        let context = ProjectContext::resolve(Some(central.clone()), None);
        assert_eq!(
            context.hooks.on_init,
            Some(HookCmd::Shell("central".to_owned()))
        );

        // .sayuki on_init overrides central.
        let sayuki = SayukiProject {
            on_init: Some(HookCmd::Shell("project".to_owned())),
            ..SayukiProject::default()
        };
        let context = ProjectContext::resolve(Some(central), Some(sayuki));
        assert_eq!(
            context.hooks.on_init,
            Some(HookCmd::Shell("project".to_owned()))
        );
    }

    #[test]
    fn trust_store_gates_on_path_and_content_hash() {
        let path = PathBuf::from("/p/.sayuki");
        let content = "apps = [\"ghostty\"]\n";
        let allowlist = format!("{} {}\n", content_hash(content), path.display());
        let store = TrustStore::parse(&allowlist);

        assert!(store.is_trusted(&path, content));
        // Editing the file changes the hash and re-blocks it.
        assert!(!store.is_trusted(&path, "apps = [\"evil\"]\n"));
        // An unlisted path is never trusted.
        assert!(!store.is_trusted(Path::new("/other/.sayuki"), content));
    }

    #[test]
    fn trust_store_parse_skips_comments_and_blanks() {
        let store = TrustStore::parse("# header\n\n  abcd /p/.sayuki  \n");
        assert!(store.is_trusted(Path::new("/p/.sayuki"), "x") == (content_hash("x") == "abcd"));
        assert_eq!(
            store
                .entries
                .get(Path::new("/p/.sayuki"))
                .map(String::as_str),
            Some("abcd")
        );
    }

    #[test]
    fn content_hash_is_stable_and_distinguishes() {
        assert_eq!(content_hash("a"), content_hash("a"));
        assert_ne!(content_hash("a"), content_hash("b"));
    }
}
