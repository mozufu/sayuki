use std::process::{Child, Command};

use tracing::{debug, info, warn};

#[derive(Debug, Default)]
pub(crate) struct ActionRunner {
    wayland_display: Option<String>,
    children: Vec<Child>,
}

impl ActionRunner {
    pub(crate) fn set_wayland_display(&mut self, wayland_display: String) {
        self.wayland_display = Some(wayland_display);
    }

    pub(crate) fn spawn(&mut self, argv: &[String]) {
        let Some(program) = argv.first() else {
            warn!("ignored empty spawn action");
            return;
        };

        let mut command = Command::new(program);
        command
            .args(&argv[1..])
            .env("GDK_BACKEND", "wayland")
            .env_remove("DISPLAY");
        if let Some(wayland_display) = &self.wayland_display {
            command.env("WAYLAND_DISPLAY", wayland_display);
        }

        match command.spawn() {
            Ok(child) => {
                info!(pid = child.id(), command = ?argv, "spawned command");
                self.children.push(child);
            }
            Err(error) => warn!(?error, command = ?argv, "failed to spawn command"),
        }
    }

    pub(crate) fn reap_children(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                debug!(pid = child.id(), ?status, "spawned child exited");
                false
            }
            Ok(None) => true,
            Err(error) => {
                warn!(pid = child.id(), ?error, "failed to reap spawned child");
                false
            }
        });
    }
}
