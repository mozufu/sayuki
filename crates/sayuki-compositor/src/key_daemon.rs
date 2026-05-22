use std::process::{Child, Command};

use smithay::{
    backend::input::KeyState,
    input::keyboard::{FilterResult, Keycode, Keysym, ModifiersState},
};
use tracing::{debug, info, warn};

#[derive(Debug, Default)]
pub(crate) struct KeyDaemon {
    wayland_display: Option<String>,
    suppressed_keycodes: Vec<Keycode>,
    children: Vec<Child>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeyAction {
    None,
    SpawnGhostty,
}

impl KeyDaemon {
    pub(crate) fn set_wayland_display(&mut self, wayland_display: String) {
        self.wayland_display = Some(wayland_display);
    }

    pub(crate) fn filter_key(
        &mut self,
        keycode: Keycode,
        state: KeyState,
        modifiers: &ModifiersState,
        keysym: Keysym,
    ) -> FilterResult<KeyAction> {
        if state == KeyState::Released && self.unsuppress_key(keycode) {
            return FilterResult::Intercept(KeyAction::None);
        }

        if state == KeyState::Pressed && modifiers.alt && is_enter(keysym) {
            let first_press = self.suppress_key(keycode);
            let action = if first_press {
                KeyAction::SpawnGhostty
            } else {
                KeyAction::None
            };
            return FilterResult::Intercept(action);
        }

        FilterResult::Forward
    }

    pub(crate) fn run_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::None => {}
            KeyAction::SpawnGhostty => self.spawn_ghostty(),
        }
    }

    pub(crate) fn reap_children(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                debug!(pid = child.id(), ?status, "key daemon child exited");
                false
            }
            Ok(None) => true,
            Err(error) => {
                warn!(pid = child.id(), ?error, "failed to reap key daemon child");
                false
            }
        });
    }

    fn spawn_ghostty(&mut self) {
        let mut command = Command::new("ghostty");
        command.env("GDK_BACKEND", "wayland").env_remove("DISPLAY");
        if let Some(wayland_display) = &self.wayland_display {
            command.env("WAYLAND_DISPLAY", wayland_display);
        }

        match command.spawn() {
            Ok(child) => {
                info!(pid = child.id(), "spawned ghostty");
                self.children.push(child);
            }
            Err(error) => warn!(?error, "failed to spawn ghostty"),
        }
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

fn is_enter(keysym: Keysym) -> bool {
    matches!(keysym, Keysym::Return | Keysym::KP_Enter)
}
