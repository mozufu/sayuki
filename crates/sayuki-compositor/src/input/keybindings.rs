use smithay::{
    backend::input::KeyState,
    input::keyboard::{FilterResult, Keycode, Keysym, ModifiersState, xkb},
};

use sayuki_ipc::Action;

use crate::{config::KeybindingConfig, input::actions::action_from_config};

#[derive(Clone, Debug)]
pub(crate) struct KeybindingRegistry {
    bindings: Vec<Keybinding>,
    suppressed_keycodes: Vec<Keycode>,
}

#[derive(Clone, Debug)]
struct Keybinding {
    combo: KeyCombo,
    action: Action,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct KeyCombo {
    modifiers: KeyModifiers,
    keysym: Keysym,
    label: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct KeyModifiers {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
}

impl KeybindingRegistry {
    pub(crate) fn from_configs(configs: &[KeybindingConfig]) -> Result<Self, String> {
        let bindings = configs
            .iter()
            .map(|config| {
                Ok(Keybinding {
                    combo: KeyCombo::parse(&config.keys)?,
                    action: action_from_config(&config.action)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        Ok(Self {
            bindings,
            suppressed_keycodes: Vec::new(),
        })
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &Action)> {
        self.bindings
            .iter()
            .map(|binding| (binding.combo.label.as_str(), &binding.action))
    }

    pub(crate) fn filter_key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        modifiers: &ModifiersState,
        keysym: Keysym,
    ) -> FilterResult<Action> {
        if state == KeyState::Released && self.unsuppress_key(keycode) {
            return FilterResult::Intercept(Action::Noop);
        }

        if state != KeyState::Pressed {
            return FilterResult::Forward;
        }

        let Some(binding_action) = self
            .bindings
            .iter()
            .find(|binding| binding.combo.matches(modifiers, keysym))
            .map(|binding| binding.action.clone())
        else {
            return FilterResult::Forward;
        };

        let first_press = self.suppress_key(keycode);
        let action = if first_press {
            binding_action
        } else {
            Action::Noop
        };

        FilterResult::Intercept(action)
    }

    fn suppress_key(&mut self, keycode: Keycode) -> bool {
        if self.suppressed_keycodes.contains(&keycode) {
            return false;
        }

        self.suppressed_keycodes.push(keycode);
        true
    }

    fn unsuppress_key(&mut self, keycode: Keycode) -> bool {
        let Some(position) = self
            .suppressed_keycodes
            .iter()
            .position(|suppressed| *suppressed == keycode)
        else {
            return false;
        };

        self.suppressed_keycodes.remove(position);
        true
    }
}

impl KeyCombo {
    fn parse(input: &str) -> Result<Self, String> {
        let mut modifiers = KeyModifiers::default();
        let mut keysym = None;

        for raw_part in input.split('+') {
            let part = raw_part.trim();
            if part.is_empty() {
                return Err(format!("invalid empty keybinding segment in `{input}`"));
            }

            if modifiers.set_from_name(part) {
                continue;
            }

            if keysym.is_some() {
                return Err(format!(
                    "keybinding `{input}` contains more than one non-modifier key"
                ));
            }
            keysym = Some(
                parse_keysym(part)
                    .ok_or_else(|| format!("keybinding `{input}` uses unknown key `{part}`"))?,
            );
        }

        let Some(keysym) = keysym else {
            return Err(format!("keybinding `{input}` does not contain a key"));
        };

        Ok(Self {
            modifiers,
            keysym,
            label: normalize_label(input)?,
        })
    }

    fn matches(&self, modifiers: &ModifiersState, keysym: Keysym) -> bool {
        self.keysym == keysym
            && self.modifiers.ctrl == modifiers.ctrl
            && self.modifiers.alt == modifiers.alt
            && self.modifiers.shift == modifiers.shift
            && self.modifiers.logo == modifiers.logo
    }
}

impl KeyModifiers {
    fn set_from_name(&mut self, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => self.ctrl = true,
            "alt" | "mod1" => self.alt = true,
            "shift" => self.shift = true,
            "super" | "logo" | "meta" | "mod4" => self.logo = true,
            _ => return false,
        }

        true
    }
}

fn parse_keysym(name: &str) -> Option<Keysym> {
    let lower = name.to_ascii_lowercase();
    let canonical = match lower.as_str() {
        "enter" => "Return",
        "esc" | "escape" => "Escape",
        "backspace" => "BackSpace",
        "space" => "space",
        "tab" => "Tab",
        "slash" => "slash",
        _ => name,
    };

    let exact = keysym_from_name(canonical, 0);
    if exact != Keysym::NoSymbol {
        return Some(exact);
    }

    let without_prefix = canonical
        .strip_prefix("XK_")
        .or_else(|| canonical.strip_prefix("xk_"))
        .unwrap_or(canonical);
    let exact_without_prefix = keysym_from_name(without_prefix, 0);
    if exact_without_prefix != Keysym::NoSymbol {
        return Some(exact_without_prefix);
    }

    let case_insensitive = keysym_from_name(without_prefix, xkb::KEYSYM_CASE_INSENSITIVE);
    if case_insensitive != Keysym::NoSymbol {
        return Some(case_insensitive);
    }

    None
}

fn keysym_from_name(name: &str, flags: xkb::KeysymFlags) -> Keysym {
    xkb::keysym_from_name(name, flags)
}

fn normalize_label(input: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for raw_part in input.split('+') {
        let part = raw_part.trim();
        if part.is_empty() {
            return Err(format!("invalid empty keybinding segment in `{input}`"));
        }
        parts.push(match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "Ctrl".to_owned(),
            "alt" | "mod1" => "Alt".to_owned(),
            "shift" => "Shift".to_owned(),
            "super" | "logo" | "meta" | "mod4" => "Super".to_owned(),
            "enter" => "Enter".to_owned(),
            "esc" | "escape" => "Escape".to_owned(),
            "backspace" => "Backspace".to_owned(),
            "space" => "Space".to_owned(),
            "tab" => "Tab".to_owned(),
            "slash" => "/".to_owned(),
            _ => part.to_owned(),
        });
    }
    Ok(parts.join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_aliases() {
        assert_eq!(parse_keysym("Enter"), Some(Keysym::Return));
        assert_eq!(parse_keysym("Escape"), Some(Keysym::Escape));
        assert_eq!(parse_keysym("Backspace"), Some(Keysym::BackSpace));
    }

    #[test]
    fn parses_shifted_letters_exactly() {
        assert_eq!(parse_keysym("Q"), Some(Keysym::Q));
        assert_eq!(parse_keysym("q"), Some(Keysym::q));
    }

    #[test]
    fn labels_normalized_bindings() {
        let combo = KeyCombo::parse("Super+Shift+Slash").expect("combo");
        assert_eq!(combo.label, "Super + Shift + /");
    }
}
