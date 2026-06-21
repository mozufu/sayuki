//! Project session glue (milestone 5b) on top of the core `state.rs`.
//!
//! These methods resolve and apply project context: spawning in the active
//! canvas's cwd/env, running lifecycle hooks, applying per-output policy, and
//! the deferred window-rule routing that re-homes a freshly mapped window onto
//! its project canvas. They live in a child module so the core file stays
//! focused; as a descendant of `state` they reach the private `SayukiState`
//! fields and helpers directly.

use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};
use tracing::{debug, warn};

use super::SayukiState;
use crate::{
    input::spawn::SpawnContext,
    output,
    project::{ProjectConfig, ProjectContext, SayukiProject, TrustStore},
    wm::{CanvasId, viewport},
};

impl SayukiState {
    /// Spawn `argv` in the active canvas's project context (cwd + env overlay).
    pub(super) fn spawn_in_active(&mut self, argv: &[String]) {
        let cwd = self
            .wm
            .active()
            .working_dir()
            .map(std::path::Path::to_owned);
        let env = self.wm.active().env().to_vec();
        self.action_runner.spawn(
            argv,
            SpawnContext {
                cwd: cwd.as_deref(),
                env: &env,
            },
        );
    }

    /// Spawn each hook/app command in the given project context.
    pub(super) fn run_commands(
        &mut self,
        commands: Vec<Vec<String>>,
        cwd: Option<&std::path::Path>,
        env: &[(String, String)],
    ) {
        for argv in commands {
            self.action_runner.spawn(&argv, SpawnContext { cwd, env });
        }
    }

    /// Re-apply the per-output scale/transform policy to every current output.
    /// Idempotent: covers initial setup, hotplug, and session reactivation.
    pub(super) fn apply_output_policies(&self) {
        let policies = &self.output_policies;
        self.backend
            .for_each_output(|output| output::apply_policy(output, policies));
    }

    /// Place a window directly into a project canvas (window-rule routing): it
    /// becomes that canvas's focused window without disturbing the active canvas.
    fn place_window_in(&mut self, target: CanvasId, window: Window) {
        let region = self.primary_output_geometry();
        let location = viewport::placement_location(region, self.next_window_index);
        self.next_window_index = self.next_window_index.wrapping_add(1);
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.bounds = Some(region.size);
            });
        }
        let canvas = self.wm.canvas_mut(target);
        canvas
            .space_mut()
            .map_element(window.clone(), location, false);
        canvas.focus(window);
    }

    /// Evaluate window rules once, when a queued window's client has provided an
    /// identity (app_id/title) or attached its first buffer. A matching `pin`
    /// rule for a non-active canvas re-routes the window to that project canvas.
    pub(super) fn route_pending_window(&mut self, window: &Window, surface: &WlSurface) {
        let Some(position) = self
            .pending_rules
            .iter()
            .position(|pending| pending == window)
        else {
            return;
        };
        let (app_id, title) = window_identity(window);
        if app_id.is_none() && title.is_none() && !surface_has_buffer(surface) {
            return;
        }
        self.pending_rules.remove(position);

        if let Some(target) = self.wm.pin_target(app_id.as_deref(), title.as_deref())
            && !self.wm.is_active(target)
        {
            self.reroute_window(window, target);
        }
    }

    /// Move a mapped window to a project canvas (window-rule routing). Removal is
    /// canvas-agnostic: a still-pending window may have been relocated to another
    /// canvas by a switch/move before its first qualifying commit, so drop it
    /// from wherever it lives, then re-focus the active tail if it lost focus.
    fn reroute_window(&mut self, window: &Window, target: CanvasId) {
        let was_active_focus = self.wm.remove_window(window);
        self.place_window_in(target, window.clone());
        if was_active_focus {
            let focus = self.wm.active().focused().cloned();
            self.apply_focus(focus);
        }
    }
}

/// Whether `surface` has a buffer attached — a reliable "the window is about to
/// be shown" signal even when a client never sets app_id/title.
fn surface_has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

/// The committed `app_id`/`title` of a toplevel window, for window-rule matching.
fn window_identity(window: &Window) -> (Option<String>, Option<String>) {
    let Some(toplevel) = window.toplevel() else {
        return (None, None);
    };
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .map(|attributes| (attributes.app_id.clone(), attributes.title.clone()))
            .unwrap_or((None, None))
    })
}

/// Resolve each central `[[project]]` into a named project context, merging a
/// discovered `.sayuki` only when the trust gate allows it.
pub(super) fn resolve_project_contexts(
    projects: &[ProjectConfig],
) -> Vec<(String, ProjectContext)> {
    let trust = TrustStore::load();
    projects
        .iter()
        .map(|config| {
            let sayuki = SayukiProject::discover(&config.path).and_then(|(path, content)| {
                if trust.is_trusted(&path, &content) {
                    match SayukiProject::parse(&content) {
                        Ok(project) => Some(project),
                        Err(error) => {
                            warn!(?error, path = ?path, "ignoring malformed .sayuki");
                            None
                        }
                    }
                } else {
                    debug!(path = ?path, "ignoring untrusted .sayuki; run allow to enable");
                    None
                }
            });
            let context = ProjectContext::resolve(Some(config.clone()), sayuki);
            (config.name.clone(), context)
        })
        .collect()
}
