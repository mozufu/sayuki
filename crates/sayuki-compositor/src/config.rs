use std::{
    error::Error,
    fs,
    io::{self, ErrorKind},
    path::Path,
};

use serde::Deserialize;
use smithay::input::keyboard::XkbConfig;

const DEFAULT_REPEAT_DELAY: i32 = 500;
const DEFAULT_REPEAT_RATE: i32 = 25;

#[derive(Clone, Debug)]
pub(crate) struct SayukiConfig {
    pub(crate) keyboard: KeyboardConfig,
    pub(crate) keybindings: Vec<KeybindingConfig>,
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
    SwitchWorkspace { workspace: u8 },
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    keyboard: KeyboardConfig,
    keybindings: Option<Vec<RawKeybindingConfig>>,
}

#[derive(Debug, Deserialize)]
struct RawKeybindingConfig {
    keys: String,
    action: String,
    command: Option<CommandConfig>,
    edges: Option<String>,
    workspace: Option<u8>,
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

        Ok(SayukiConfig {
            keyboard: self.keyboard,
            keybindings,
        })
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
            "switch-workspace" | "workspace" => {
                let Some(workspace) = self.workspace else {
                    return Err(invalid_keybinding(format!(
                        "keybinding `{}` uses action `{}` without `workspace`",
                        self.keys, self.action
                    )));
                };
                BindingActionConfig::SwitchWorkspace { workspace }
            }
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
    vec![
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
    ]
}

fn invalid_keybinding(message: String) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}
