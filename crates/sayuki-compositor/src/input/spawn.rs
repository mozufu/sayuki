use std::{
    path::Path,
    process::{Child, Command},
};

use tracing::{debug, info, warn};

/// Per-spawn project context: where to run a child and which env to overlay.
/// Default (no cwd, empty env) reproduces the pre-5b behavior.
#[derive(Clone, Copy, Default)]
pub(crate) struct SpawnContext<'a> {
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) env: &'a [(String, String)],
}

#[derive(Debug, Default)]
pub(crate) struct ActionRunner {
    wayland_display: Option<String>,
    ipc_socket: Option<String>,
    direnv_available: bool,
    children: Vec<Child>,
}

impl ActionRunner {
    pub(crate) fn new() -> Self {
        Self {
            direnv_available: direnv_available(),
            ..Self::default()
        }
    }

    pub(crate) fn set_wayland_display(&mut self, wayland_display: String) {
        self.wayland_display = Some(wayland_display);
    }

    pub(crate) fn set_ipc_socket(&mut self, ipc_socket: String) {
        self.ipc_socket = Some(ipc_socket);
    }

    pub(crate) fn spawn(&mut self, argv: &[String], context: SpawnContext<'_>) {
        let Some(mut command) = build_command(
            argv,
            context,
            self.direnv_available,
            self.wayland_display.as_deref(),
            self.ipc_socket.as_deref(),
        ) else {
            warn!("ignored empty spawn action");
            return;
        };

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

/// Build the command for `argv`. With a project `cwd` and direnv available, wrap
/// in `direnv exec <dir> …` so the child inherits the project `.envrc`; with the
/// cwd but no direnv, run the program directly in `dir`; with neither, run it as
/// before. Env precedence is inherited → canvas overlay → fixed compositor vars,
/// so the overlay can never drop `WAYLAND_DISPLAY` or `SAYUKI_SOCKET`. Returns
/// `None` for empty argv.
fn build_command(
    argv: &[String],
    context: SpawnContext<'_>,
    direnv_available: bool,
    wayland_display: Option<&str>,
    ipc_socket: Option<&str>,
) -> Option<Command> {
    let (program, rest) = argv.split_first()?;

    let mut command = match context.cwd {
        Some(dir) if direnv_available => {
            let mut command = Command::new("direnv");
            command.arg("exec").arg(dir).arg(program).args(rest);
            // Set the cwd ourselves rather than relying on exec's chdir, and
            // silence the "direnv: loading…" banner.
            command.current_dir(dir);
            command.env("DIRENV_LOG_FORMAT", "");
            command
        }
        Some(dir) => {
            let mut command = Command::new(program);
            command.args(rest);
            command.current_dir(dir);
            command
        }
        None => {
            let mut command = Command::new(program);
            command.args(rest);
            command
        }
    };

    for (key, value) in context.env {
        command.env(key, value);
    }
    command.env("GDK_BACKEND", "wayland").env_remove("DISPLAY");
    if let Some(wayland_display) = wayland_display {
        command.env("WAYLAND_DISPLAY", wayland_display);
    }
    if let Some(ipc_socket) = ipc_socket {
        command.env(crate::ipc::SOCKET_ENV, ipc_socket);
    }

    Some(command)
}

/// Whether a `direnv` executable is on `PATH`. direnv is a soft runtime
/// dependency: when absent, spawns fall back to a direct launch.
fn direnv_available() -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| dir.join("direnv").is_file())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, ffi::OsStr};

    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn envs(command: &Command) -> HashMap<String, String> {
        command
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    #[test]
    fn direnv_wraps_command_with_project_cwd() {
        let context = SpawnContext {
            cwd: Some(Path::new("/p")),
            env: &[],
        };
        let command = build_command(
            &["ghostty".to_owned()],
            context,
            true,
            Some("wayland-1"),
            None,
        )
        .expect("cmd");
        assert_eq!(command.get_program(), OsStr::new("direnv"));
        assert_eq!(args(&command), ["exec", "/p", "ghostty"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/p")));
    }

    #[test]
    fn direnv_absent_runs_program_directly_in_cwd() {
        let context = SpawnContext {
            cwd: Some(Path::new("/p")),
            env: &[],
        };
        let command = build_command(
            &["ghostty".to_owned()],
            context,
            false,
            Some("wayland-1"),
            None,
        )
        .expect("cmd");
        assert_eq!(command.get_program(), OsStr::new("ghostty"));
        assert!(args(&command).is_empty());
        assert_eq!(command.get_current_dir(), Some(Path::new("/p")));
    }

    #[test]
    fn no_project_context_runs_program_without_cwd() {
        let command = build_command(
            &["ghostty".to_owned(), "--flag".to_owned()],
            SpawnContext::default(),
            true,
            None,
            None,
        )
        .expect("cmd");
        assert_eq!(command.get_program(), OsStr::new("ghostty"));
        assert_eq!(args(&command), ["--flag"]);
        assert_eq!(command.get_current_dir(), None);
    }

    #[test]
    fn env_overlay_never_drops_wayland_display() {
        let overlay = vec![
            ("WAYLAND_DISPLAY".to_owned(), "stale".to_owned()),
            ("SAYUKI_SOCKET".to_owned(), "stale".to_owned()),
            ("RUST_LOG".to_owned(), "debug".to_owned()),
        ];
        let context = SpawnContext {
            cwd: None,
            env: &overlay,
        };
        let command = build_command(
            &["ghostty".to_owned()],
            context,
            false,
            Some("wayland-1"),
            Some("/tmp/sayuki.sock"),
        )
        .expect("cmd");
        let envs = envs(&command);
        // The fixed Wayland var is applied after the overlay, so it wins.
        assert_eq!(
            envs.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("wayland-1")
        );
        assert_eq!(
            envs.get("SAYUKI_SOCKET").map(String::as_str),
            Some("/tmp/sayuki.sock")
        );
        assert_eq!(envs.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert_eq!(envs.get("GDK_BACKEND").map(String::as_str), Some("wayland"));
    }

    #[test]
    fn empty_argv_builds_no_command() {
        assert!(build_command(&[], SpawnContext::default(), true, None, None).is_none());
    }
}
