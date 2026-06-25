use std::{
    error::Error,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use smithay::input::keyboard::XkbConfig;

use crate::{
    output::OutputPolicy,
    project::ProjectConfig,
    wm::{PanCouple, WorkspaceRef, snap::SnapConfig, tiling::TilingConfig},
};

const DEFAULT_REPEAT_DELAY: i32 = 500;
const DEFAULT_REPEAT_RATE: i32 = 25;
const DEFAULT_PAN_STEP: i32 = 200;
const DEFAULT_ZOOM_IN: f64 = 1.1;
const DEFAULT_ZOOM_OUT: f64 = 0.9;

#[derive(Clone, Debug)]
pub(crate) struct SayukiConfig {
    pub(crate) keyboard: KeyboardConfig,
    pub(crate) keybindings: Vec<KeybindingConfig>,
    pub(crate) pan_couple: PanCouple,
    pub(crate) snap: SnapConfig,
    pub(crate) tiling: TilingConfig,
    pub(crate) projects: Vec<ProjectConfig>,
    pub(crate) outputs: Vec<OutputPolicy>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct KeyboardConfig {
    pub(crate) rules: String,
    pub(crate) model: String,
    pub(crate) layout: String,
    pub(crate) variant: String,
    pub(crate) options: Option<String>,
    pub(crate) repeat_delay: i32,
    pub(crate) repeat_rate: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct KeybindingConfig {
    pub(crate) keys: String,
    pub(crate) action: BindingActionConfig,
}

#[derive(Clone, Debug)]
pub(crate) enum BindingActionConfig {
    Quit,
    Spawn { command: Vec<String> },
    BeginMove,
    BeginResize { edges: String },
    SwitchWorkspace { workspace: WorkspaceRef },
    MoveToWorkspace { workspace: WorkspaceRef },
    PanViewport { dx: i32, dy: i32 },
    ZoomViewport { factor: f64 },
    ToggleOverview,
    ToggleMinimap,
    TogglePin,
    SwapWindow { target: String },
    FocusNext,
    FocusPrev,
    FocusTile { direction: String },
    MoveTile { direction: String },
    ToggleFloating,
    ToggleTiling,
    ToggleHelp,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    keyboard: KeyboardConfig,
    keybindings: Option<Vec<RawKeybindingConfig>>,
    #[serde(default)]
    pan: RawPanConfig,
    #[serde(default)]
    snap: RawSnapConfig,
    #[serde(default)]
    tiling: RawTilingConfig,
    project: Option<Vec<RawProject>>,
    output: Option<Vec<RawOutput>>,
}

#[derive(Debug, Deserialize)]
struct RawKeybindingConfig {
    keys: String,
    action: RawAction,
}

/// Zutai tagged-union JSON: bare atoms arrive as `"#tag"` strings; variants
/// with payloads arrive as `{ "tag": "name", "payload": { ... } }`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawAction {
    Atom(String),
    Tagged {
        tag: String,
        payload: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct RawProject {
    name: String,
    path: String,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    on_init: Option<crate::project::HookCmd>,
    layout: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawOutput {
    name: String,
    scale: Option<i32>,
    transform: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawPanConfig {
    couple: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawSnapConfig {
    threshold: i32,
    grid: i32,
    to_windows: bool,
    to_edges: bool,
}

impl Default for RawSnapConfig {
    fn default() -> Self {
        let d = crate::wm::snap::SnapConfig::default();
        Self {
            threshold: d.threshold,
            grid: d.grid,
            to_windows: d.to_windows,
            to_edges: d.to_edges,
        }
    }
}

impl RawSnapConfig {
    fn into_snap(self) -> crate::wm::snap::SnapConfig {
        crate::wm::snap::SnapConfig {
            threshold: self.threshold,
            grid: self.grid,
            to_windows: self.to_windows,
            to_edges: self.to_edges,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawTilingConfig {
    gap: i32,
}

impl Default for RawTilingConfig {
    fn default() -> Self {
        Self {
            gap: TilingConfig::default().gap,
        }
    }
}

impl RawTilingConfig {
    fn into_tiling(self) -> TilingConfig {
        TilingConfig {
            gap: self.gap.max(0),
        }
    }
}

impl SayukiConfig {
    /// Load compositor config from the first found source:
    /// 1. `explicit` (`--config` flag)
    /// 2. `$XDG_CONFIG_HOME/sayuki/config.zt` (user)
    /// 3. `/etc/sayuki/config.zt` (system)
    ///
    /// Returns the config and the path that was loaded (None = built-in defaults).
    /// The path is what callers should watch for hot-reload.
    pub(crate) fn load(explicit: Option<&Path>) -> Result<(Self, Option<PathBuf>), Box<dyn Error>> {
        let path = explicit
            .map(ToOwned::to_owned)
            .or_else(find_user_config)
            .or_else(find_system_config);

        let Some(path) = path else {
            return Ok((Self::default(), None));
        };

        let cfg = Self::load_from(&path)?;
        Ok((cfg, Some(path)))
    }

    /// Evaluate a specific `.zt` file without path discovery.  Used on hot-reload.
    pub(crate) fn load_from(path: &Path) -> Result<Self, Box<dyn Error>> {
        let json = zutai_eval::eval_path_to_json(path)?;
        let raw: RawConfig = serde_json::from_value(json)?;
        raw.try_into_config()
    }
}

fn find_user_config() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let path = base.join("sayuki").join("config.zt");
    path.exists().then_some(path)
}

fn find_system_config() -> Option<PathBuf> {
    let path = PathBuf::from("/etc/sayuki/config.zt");
    path.exists().then_some(path)
}

impl Default for SayukiConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            keybindings: default_keybindings(),
            pan_couple: PanCouple::default(),
            snap: SnapConfig::default(),
            tiling: TilingConfig::default(),
            projects: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            rules: String::new(),
            model: String::new(),
            layout: String::new(),
            variant: String::new(),
            options: None,
            repeat_delay: DEFAULT_REPEAT_DELAY,
            repeat_rate: DEFAULT_REPEAT_RATE,
        }
    }
}

impl KeyboardConfig {
    pub(crate) fn xkb_config(&self) -> XkbConfig<'_> {
        XkbConfig {
            rules: &self.rules,
            model: &self.model,
            layout: &self.layout,
            variant: &self.variant,
            options: self
                .options
                .as_ref()
                .filter(|options| !options.is_empty())
                .cloned(),
        }
    }
}

impl RawConfig {
    fn try_into_config(self) -> Result<SayukiConfig, Box<dyn Error>> {
        let keybindings = match self.keybindings {
            Some(bindings) => bindings
                .into_iter()
                .map(RawKeybindingConfig::try_into_config)
                .collect::<Result<Vec<_>, _>>()?,
            None => default_keybindings(),
        };

        let projects = self
            .project
            .unwrap_or_default()
            .into_iter()
            .map(RawProject::into_project)
            .collect();
        let outputs = self
            .output
            .unwrap_or_default()
            .into_iter()
            .map(RawOutput::into_policy)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(SayukiConfig {
            keyboard: self.keyboard,
            keybindings,
            pan_couple: self.pan.into_couple()?,
            snap: self.snap.into_snap(),
            tiling: self.tiling.into_tiling(),
            projects,
            outputs,
        })
    }
}

impl RawPanConfig {
    fn into_couple(self) -> Result<PanCouple, io::Error> {
        let Some(couple) = self.couple else {
            return Ok(PanCouple::default());
        };
        match couple
            .trim_start_matches('#')
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "independent" => Ok(PanCouple::Independent),
            "linked" => Ok(PanCouple::Linked),
            other => Err(invalid_keybinding(format!(
                "unknown pan.couple `{other}`; expected `independent` or `linked`"
            ))),
        }
    }
}

impl RawKeybindingConfig {
    fn try_into_config(self) -> Result<KeybindingConfig, io::Error> {
        let action = self.action.try_into_binding_action(&self.keys)?;
        Ok(KeybindingConfig {
            keys: self.keys,
            action,
        })
    }
}

impl RawAction {
    fn try_into_binding_action(self, keys: &str) -> Result<BindingActionConfig, io::Error> {
        // Helper: pull a field from a payload object.
        fn str_field<'a>(
            p: &'a serde_json::Value,
            field: &str,
            keys: &str,
        ) -> Result<&'a str, io::Error> {
            p.get(field).and_then(|v| v.as_str()).ok_or_else(|| {
                invalid_keybinding(format!("keybinding `{keys}`: missing `{field}`"))
            })
        }
        fn i32_field(p: &serde_json::Value, field: &str, keys: &str) -> Result<i32, io::Error> {
            p.get(field)
                .and_then(|v| v.as_i64())
                .map(|n| n as i32)
                .ok_or_else(|| {
                    invalid_keybinding(format!("keybinding `{keys}`: missing `{field}`"))
                })
        }
        fn f64_field(p: &serde_json::Value, field: &str, keys: &str) -> Result<f64, io::Error> {
            p.get(field).and_then(|v| v.as_f64()).ok_or_else(|| {
                invalid_keybinding(format!("keybinding `{keys}`: missing `{field}`"))
            })
        }

        match self {
            // ── zero-payload atoms ──────────────────────────────────────────
            RawAction::Atom(atom) => match atom.trim_start_matches('#') {
                "quit" => Ok(BindingActionConfig::Quit),
                "begin_move" | "move" => Ok(BindingActionConfig::BeginMove),
                "toggle_overview" | "overview" => Ok(BindingActionConfig::ToggleOverview),
                "toggle_minimap" | "minimap" => Ok(BindingActionConfig::ToggleMinimap),
                "toggle_pin" | "pin" => Ok(BindingActionConfig::TogglePin),
                "focus_next" => Ok(BindingActionConfig::FocusNext),
                "focus_prev" | "focus_previous" => Ok(BindingActionConfig::FocusPrev),
                "toggle_floating" | "float" => Ok(BindingActionConfig::ToggleFloating),
                "toggle_tiling" | "tiling" => Ok(BindingActionConfig::ToggleTiling),
                "toggle_help" | "help" => Ok(BindingActionConfig::ToggleHelp),
                other => Err(invalid_keybinding(format!(
                    "keybinding `{keys}` uses unknown action `{other}`"
                ))),
            },

            // ── tagged variants with payloads ───────────────────────────────
            RawAction::Tagged { tag, payload: p } => match tag.trim_start_matches('#') {
                "spawn" => {
                    let cmd = str_field(&p, "command", keys)?;
                    if cmd.is_empty() {
                        return Err(invalid_keybinding(format!(
                            "keybinding `{keys}`: empty spawn command"
                        )));
                    }
                    Ok(BindingActionConfig::Spawn {
                        command: vec!["sh".to_owned(), "-c".to_owned(), cmd.to_owned()],
                    })
                }
                "spawn_args" => {
                    let args: Vec<String> = serde_json::from_value(
                        p.get("args")
                            .cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    )
                    .map_err(|e| {
                        invalid_keybinding(format!("keybinding `{keys}` spawn_args: {e}"))
                    })?;
                    if args.is_empty() || args.iter().any(|a| a.is_empty()) {
                        return Err(invalid_keybinding(format!(
                            "keybinding `{keys}`: empty spawn_args"
                        )));
                    }
                    Ok(BindingActionConfig::Spawn { command: args })
                }
                "begin_resize" | "resize" => {
                    let edges = p
                        .get("edges")
                        .and_then(|v| v.as_str())
                        .unwrap_or("bottom-right")
                        .to_owned();
                    Ok(BindingActionConfig::BeginResize { edges })
                }
                "switch_workspace_index" => Ok(BindingActionConfig::SwitchWorkspace {
                    workspace: WorkspaceRef::Index(i32_field(&p, "n", keys)? as u8),
                }),
                "switch_workspace_name" => Ok(BindingActionConfig::SwitchWorkspace {
                    workspace: WorkspaceRef::Name(str_field(&p, "name", keys)?.to_owned()),
                }),
                "move_to_workspace_index" => Ok(BindingActionConfig::MoveToWorkspace {
                    workspace: WorkspaceRef::Index(i32_field(&p, "n", keys)? as u8),
                }),
                "move_to_workspace_name" => Ok(BindingActionConfig::MoveToWorkspace {
                    workspace: WorkspaceRef::Name(str_field(&p, "name", keys)?.to_owned()),
                }),
                "pan" => {
                    let dx = i32_field(&p, "dx", keys)?;
                    let dy = i32_field(&p, "dy", keys)?;
                    if dx == 0 && dy == 0 {
                        return Err(invalid_keybinding(format!(
                            "keybinding `{keys}`: pan requires non-zero dx or dy"
                        )));
                    }
                    Ok(BindingActionConfig::PanViewport { dx, dy })
                }
                "zoom" => {
                    let factor = f64_field(&p, "factor", keys)?;
                    if !(factor.is_finite() && factor > 0.0) {
                        return Err(invalid_keybinding(format!(
                            "keybinding `{keys}`: zoom factor must be positive"
                        )));
                    }
                    Ok(BindingActionConfig::ZoomViewport { factor })
                }
                "swap_window" | "swap" => Ok(BindingActionConfig::SwapWindow {
                    target: str_field(&p, "target", keys)?.to_owned(),
                }),
                "focus_tile" => Ok(BindingActionConfig::FocusTile {
                    direction: str_field(&p, "direction", keys)?.to_owned(),
                }),
                "move_tile" => Ok(BindingActionConfig::MoveTile {
                    direction: str_field(&p, "direction", keys)?.to_owned(),
                }),
                other => Err(invalid_keybinding(format!(
                    "keybinding `{keys}` uses unknown action `{other}`"
                ))),
            },
        }
    }
}

impl RawProject {
    fn into_project(self) -> ProjectConfig {
        ProjectConfig {
            name: self.name,
            path: expand_tilde(&self.path),
            env: self.env.into_iter().collect(),
            on_init: self.on_init,
            layout: self.layout.as_deref().map(crate::project::parse_layout),
        }
    }
}

impl RawOutput {
    fn into_policy(self) -> Result<OutputPolicy, io::Error> {
        OutputPolicy::new(self.name, self.scale, self.transform.as_deref())
            .map_err(invalid_keybinding)
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

fn default_keybindings() -> Vec<KeybindingConfig> {
    let mut bindings = vec![
        KeybindingConfig {
            keys: "Alt+Enter".to_owned(),
            action: BindingActionConfig::Spawn {
                command: vec!["ghostty".to_owned()],
            },
        },
        KeybindingConfig {
            keys: "Alt+Shift+Q".to_owned(),
            action: BindingActionConfig::Quit,
        },
        KeybindingConfig {
            keys: "Super+Shift+Slash".to_owned(),
            action: BindingActionConfig::ToggleHelp,
        },
    ];

    // Canvas/viewport bindings so the WM model is usable for manual testing.
    for workspace in 1..=4u8 {
        bindings.push(KeybindingConfig {
            keys: format!("Alt+{workspace}"),
            action: BindingActionConfig::SwitchWorkspace {
                workspace: WorkspaceRef::Index(workspace),
            },
        });
        bindings.push(KeybindingConfig {
            keys: format!("Alt+Shift+{workspace}"),
            action: BindingActionConfig::MoveToWorkspace {
                workspace: WorkspaceRef::Index(workspace),
            },
        });
    }

    bindings.extend([
        pan_binding("Alt+Left", -DEFAULT_PAN_STEP, 0),
        pan_binding("Alt+Right", DEFAULT_PAN_STEP, 0),
        pan_binding("Alt+Up", 0, -DEFAULT_PAN_STEP),
        pan_binding("Alt+Down", 0, DEFAULT_PAN_STEP),
        KeybindingConfig {
            keys: "Alt+Equal".to_owned(),
            action: BindingActionConfig::ZoomViewport {
                factor: DEFAULT_ZOOM_IN,
            },
        },
        KeybindingConfig {
            keys: "Alt+Minus".to_owned(),
            action: BindingActionConfig::ZoomViewport {
                factor: DEFAULT_ZOOM_OUT,
            },
        },
        KeybindingConfig {
            keys: "Alt+O".to_owned(),
            action: BindingActionConfig::ToggleOverview,
        },
        KeybindingConfig {
            keys: "Alt+M".to_owned(),
            action: BindingActionConfig::ToggleMinimap,
        },
        KeybindingConfig {
            keys: "Alt+P".to_owned(),
            action: BindingActionConfig::TogglePin,
        },
        KeybindingConfig {
            keys: "Alt+Tab".to_owned(),
            action: BindingActionConfig::FocusNext,
        },
        KeybindingConfig {
            keys: "Alt+Shift+Tab".to_owned(),
            action: BindingActionConfig::FocusPrev,
        },
    ]);

    // Tiling actions (vim-style): navigate and move within the column layout,
    // plus per-window float and per-canvas tiling toggles.
    bindings.extend([
        tile_focus("Alt+H", "left"),
        tile_focus("Alt+L", "right"),
        tile_focus("Alt+K", "up"),
        tile_focus("Alt+J", "down"),
        tile_move("Alt+Shift+H", "left"),
        tile_move("Alt+Shift+L", "right"),
        tile_move("Alt+Shift+K", "up"),
        tile_move("Alt+Shift+J", "down"),
        KeybindingConfig {
            keys: "Alt+T".to_owned(),
            action: BindingActionConfig::ToggleTiling,
        },
        KeybindingConfig {
            keys: "Alt+Shift+F".to_owned(),
            action: BindingActionConfig::ToggleFloating,
        },
    ]);

    bindings
}

fn pan_binding(keys: &str, dx: i32, dy: i32) -> KeybindingConfig {
    KeybindingConfig {
        keys: keys.to_owned(),
        action: BindingActionConfig::PanViewport { dx, dy },
    }
}

fn tile_focus(keys: &str, direction: &str) -> KeybindingConfig {
    KeybindingConfig {
        keys: keys.to_owned(),
        action: BindingActionConfig::FocusTile {
            direction: direction.to_owned(),
        },
    }
}

fn tile_move(keys: &str, direction: &str) -> KeybindingConfig {
    KeybindingConfig {
        keys: keys.to_owned(),
        action: BindingActionConfig::MoveTile {
            direction: direction.to_owned(),
        },
    }
}

fn invalid_keybinding(message: String) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}
