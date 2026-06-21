use std::{
    collections::HashMap,
    error::Error,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use smithay::input::keyboard::XkbConfig;

use crate::{
    output::OutputPolicy,
    project::{HookCmd, ProjectConfig},
    wm::{PanCouple, WorkspaceRef, snap::SnapConfig},
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
    project: Option<Vec<RawProject>>,
    output: Option<Vec<RawOutput>>,
}

#[derive(Debug, Deserialize)]
struct RawKeybindingConfig {
    keys: String,
    action: String,
    command: Option<CommandConfig>,
    edges: Option<String>,
    workspace: Option<RawWorkspaceRef>,
    dx: Option<i32>,
    dy: Option<i32>,
    factor: Option<f64>,
    target: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawWorkspaceRef {
    Index(u8),
    Name(String),
}

#[derive(Debug, Deserialize)]
struct RawProject {
    name: String,
    path: String,
    #[serde(default)]
    env: HashMap<String, String>,
    on_init: Option<HookCmd>,
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
        let defaults = SnapConfig::default();
        Self {
            threshold: defaults.threshold,
            grid: defaults.grid,
            to_windows: defaults.to_windows,
            to_edges: defaults.to_edges,
        }
    }
}

impl RawSnapConfig {
    fn into_snap(self) -> SnapConfig {
        SnapConfig {
            threshold: self.threshold,
            grid: self.grid,
            to_windows: self.to_windows,
            to_edges: self.to_edges,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CommandConfig {
    Shell(String),
    Args(Vec<String>),
}

impl SayukiConfig {
    pub(crate) fn load(path: Option<&Path>) -> Result<Self, Box<dyn Error>> {
        let Some(path) = path else {
            return Ok(Self::default());
        };

        let contents = fs::read_to_string(path)?;
        let raw: RawConfig = toml::from_str(&contents)?;
        raw.try_into_config()
    }
}

impl Default for SayukiConfig {
    fn default() -> Self {
        Self {
            keyboard: KeyboardConfig::default(),
            keybindings: default_keybindings(),
            pan_couple: PanCouple::default(),
            snap: SnapConfig::default(),
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
        match couple.trim().to_ascii_lowercase().as_str() {
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
        let action_name = self.action.trim().to_ascii_lowercase();
        let action = match action_name.as_str() {
            "quit" => BindingActionConfig::Quit,
            "spawn" => {
                let Some(command) = self.command else {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses action `spawn` without `command`",
                        self.keys
                    )));
                };
                let command = command.into_args();
                if command.is_empty() || command.iter().any(|arg| arg.is_empty()) {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` has an empty spawn command",
                        self.keys
                    )));
                }
                BindingActionConfig::Spawn { command }
            }
            "move" | "begin-move" => BindingActionConfig::BeginMove,
            "resize" | "begin-resize" => BindingActionConfig::BeginResize {
                edges: self.edges.unwrap_or_else(|| "bottom-right".to_owned()),
            },
            "switch-workspace" | "workspace" => BindingActionConfig::SwitchWorkspace {
                workspace: self.require_workspace()?,
            },
            "move-to-workspace" | "move-window-to-workspace" => {
                BindingActionConfig::MoveToWorkspace {
                    workspace: self.require_workspace()?,
                }
            }
            "pan" => {
                let dx = self.dx.unwrap_or(0);
                let dy = self.dy.unwrap_or(0);
                if dx == 0 && dy == 0 {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses action `pan` without a non-zero `dx`/`dy`",
                        self.keys
                    )));
                }
                BindingActionConfig::PanViewport { dx, dy }
            }
            "zoom" => {
                let Some(factor) = self.factor else {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses action `zoom` without `factor`",
                        self.keys
                    )));
                };
                if !(factor.is_finite() && factor > 0.0) {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses a non-positive zoom `factor`",
                        self.keys
                    )));
                }
                BindingActionConfig::ZoomViewport { factor }
            }
            "overview" | "toggle-overview" => BindingActionConfig::ToggleOverview,
            "minimap" | "toggle-minimap" => BindingActionConfig::ToggleMinimap,
            "pin" | "toggle-pin" => BindingActionConfig::TogglePin,
            "swap" | "swap-window" => {
                let Some(target) = self.target else {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses action `swap` without `target`",
                        self.keys
                    )));
                };
                BindingActionConfig::SwapWindow { target }
            }
            "focus-next" => BindingActionConfig::FocusNext,
            "focus-prev" | "focus-previous" => BindingActionConfig::FocusPrev,
            _ => {
                return Err(invalid_keybinding(format!(
                    "keybinding `{}` uses unknown action `{}`",
                    self.keys, self.action
                )));
            }
        };

        Ok(KeybindingConfig {
            keys: self.keys,
            action,
        })
    }

    fn require_workspace(&self) -> Result<WorkspaceRef, io::Error> {
        self.workspace
            .as_ref()
            .map(RawWorkspaceRef::to_ref)
            .ok_or_else(|| {
                invalid_keybinding(format!(
                    "keybinding `{}` uses action `{}` without `workspace`",
                    self.keys, self.action
                ))
            })
    }
}

impl RawWorkspaceRef {
    fn to_ref(&self) -> WorkspaceRef {
        match self {
            RawWorkspaceRef::Index(index) => WorkspaceRef::Index(*index),
            RawWorkspaceRef::Name(name) => WorkspaceRef::Name(name.clone()),
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

impl CommandConfig {
    fn into_args(self) -> Vec<String> {
        match self {
            CommandConfig::Shell(command) => vec!["sh".to_owned(), "-c".to_owned(), command],
            CommandConfig::Args(args) => args,
        }
    }
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

    bindings
}

fn pan_binding(keys: &str, dx: i32, dy: i32) -> KeybindingConfig {
    KeybindingConfig {
        keys: keys.to_owned(),
        action: BindingActionConfig::PanViewport { dx, dy },
    }
}

fn invalid_keybinding(message: String) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}
