//! `wlr-foreign-toplevel-management-unstable-v1`: let external taskbars/docks
//! (waybar, sfwbar, wlrctl) enumerate windows and request actions on them —
//! activate (raise + focus), close, fullscreen, and maximize.
//!
//! Smithay 0.7 ships only the *list* side (`foreign_toplevel_list`, used for
//! `ext-foreign-toplevel-list`); the wlr *management* protocol has no helper, so
//! this module hand-writes the `GlobalDispatch`/`Dispatch` glue for the manager
//! and handle objects, mirroring the [`crate::screencopy`] precedent. The
//! requested actions are mapped onto Sayuki's existing WM policy: `activate`
//! reuses [`SayukiState::focus_window`] (raise + reveal + keyboard focus),
//! `close` sends `xdg_toplevel.close`, and the fullscreen/maximize requests are
//! routed through the existing [`XdgShellHandler`] paths.
//!
//! Unlike the list protocol (where each window has a single shared handle), the
//! wlr manager is per-client: every bound manager gets its own set of handles.
//! [`ToplevelData`] therefore tracks one handle per live manager, keyed so a
//! request can resolve back to the originating window. The `state` array
//! reflects the toplevel's committed xdg state (maximized/fullscreen) plus a
//! synthetic `activated` bit for the keyboard-focused window — taskbars treat
//! `activated` as "this is the focused window", so at most one carries it.
//!
//! Window lifecycle is driven by the same push hooks as the list protocol
//! (`register`/`refresh`/`unregister` in `state/project.rs`), with an extra
//! refresh on focus changes so the `activated` state tracks focus. `output_enter`
//! follows the window's current output; it is best-effort and reconverges on the
//! next commit (Sayuki has no per-move protocol notification), which is a
//! documented limitation shared with the screencopy damage path.

use std::collections::HashMap;

use smithay::{
    desktop::Window,
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_protocols_wlr::foreign_toplevel::v1::server::{
            zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
            zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::{ClientId, GlobalId},
            protocol::{wl_output::WlOutput, wl_surface::WlSurface},
        },
    },
    wayland::shell::xdg::XdgShellHandler,
};

use crate::state::{SayukiState, window_id, window_identity};
use crate::wm::WorkspaceRef;

/// Manager version we advertise. v3 adds the `parent` event (never sent — Sayuki
/// has no toplevel parenting policy) on top of v2's fullscreen requests.
const MANAGER_VERSION: u32 = 3;

/// Owns the `zwlr_foreign_toplevel_manager_v1` global plus every live manager
/// instance and the per-window handle bookkeeping.
pub(crate) struct ForeignToplevelManagerState {
    _global: GlobalId,
    /// Every bound manager instance; a manager is announced new windows and
    /// removed on `stop`/disconnect.
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// Tracked toplevels keyed by their surface (stable for the window's life),
    /// each holding the per-manager handles and last-sent property cache.
    toplevels: HashMap<WlSurface, ToplevelData>,
}

impl ForeignToplevelManagerState {
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global = display
            .create_global::<SayukiState, ZwlrForeignToplevelManagerV1, ()>(MANAGER_VERSION, ());
        Self {
            _global: global,
            managers: Vec::new(),
            toplevels: HashMap::new(),
        }
    }
}

/// Per-window state: the handles exposed to each manager (with the outputs each
/// has entered) plus the last-sent properties, so events are emitted only on
/// real changes.
struct ToplevelData {
    /// One handle per live manager; the value is the outputs that handle has
    /// been told the window entered (for paired `output_leave`).
    handles: HashMap<ZwlrForeignToplevelHandleV1, Vec<WlOutput>>,
    title: String,
    app_id: String,
    states: Vec<u32>,
    output: Option<Output>,
}

impl ToplevelData {
    fn new(title: String, app_id: String, states: Vec<u32>, output: Option<Output>) -> Self {
        Self {
            handles: HashMap::new(),
            title,
            app_id,
            states,
            output,
        }
    }

    /// Create a fresh handle for `manager`, announce it, and send the window's
    /// current properties so the client starts in sync.
    fn add_handle(
        &mut self,
        display: &DisplayHandle,
        manager: &ZwlrForeignToplevelManagerV1,
        surface: &WlSurface,
    ) {
        let Some(client) = manager.client() else {
            return;
        };
        let Ok(handle) = client.create_resource::<ZwlrForeignToplevelHandleV1, _, SayukiState>(
            display,
            manager.version(),
            surface.clone(),
        ) else {
            return;
        };

        manager.toplevel(&handle);
        if !self.title.is_empty() {
            handle.title(self.title.clone());
        }
        if !self.app_id.is_empty() {
            handle.app_id(self.app_id.clone());
        }
        handle.state(states_to_bytes(&self.states));

        let mut entered = Vec::new();
        if let Some(output) = &self.output {
            for wl_output in output.client_outputs(&client) {
                handle.output_enter(&wl_output);
                entered.push(wl_output);
            }
        }

        handle.done();
        self.handles.insert(handle, entered);
    }
}

/// Encode the wlr state enum values as the native-endian `u32` array the
/// protocol's `state` event expects.
fn states_to_bytes(states: &[u32]) -> Vec<u8> {
    states
        .iter()
        .flat_map(|value| value.to_ne_bytes())
        .collect()
}

/// Build the wlr state array from the toplevel's committed xdg state and whether
/// it currently holds keyboard focus. Order is irrelevant to clients; the
/// `activated` bit is synthetic (keyboard focus), so at most one window has it.
fn state_flags(maximized: bool, fullscreen: bool, activated: bool) -> Vec<u32> {
    let mut states = Vec::new();
    if maximized {
        states.push(zwlr_foreign_toplevel_handle_v1::State::Maximized as u32);
    }
    if fullscreen {
        states.push(zwlr_foreign_toplevel_handle_v1::State::Fullscreen as u32);
    }
    if activated {
        states.push(zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
    }
    states
}

impl SayukiState {
    /// Announce a freshly mapped toplevel to every live manager. The window's
    /// app_id/title are usually empty at map time; [`Self::refresh_wlr_toplevel`]
    /// fills them in on the first commits.
    pub(crate) fn register_wlr_toplevel(&mut self, window: &Window) {
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else {
            return;
        };
        let (app_id, title) = window_identity(window);
        let states = self.wlr_state_vec(window);
        let output = self.output_for_window(window);
        let display = self.display_handle.clone();
        let managers = self.foreign_toplevel_manager.managers.clone();

        let mut data = ToplevelData::new(
            title.unwrap_or_default(),
            app_id.unwrap_or_default(),
            states,
            output,
        );
        for manager in &managers {
            data.add_handle(&display, manager, &surface);
        }
        self.foreign_toplevel_manager
            .toplevels
            .insert(surface, data);
    }

    /// Push the window's current properties (title/app_id/state/output) to its
    /// handles, emitting an event only for what actually changed and finalizing
    /// with `done`.
    pub(crate) fn refresh_wlr_toplevel(&mut self, window: &Window) {
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else {
            return;
        };
        let (app_id, title) = window_identity(window);
        let app_id = app_id.unwrap_or_default();
        let title = title.unwrap_or_default();
        let states = self.wlr_state_vec(window);
        let new_output = self.output_for_window(window);

        let Some(data) = self.foreign_toplevel_manager.toplevels.get_mut(&surface) else {
            return;
        };

        let title_changed = data.title != title;
        if title_changed {
            data.title.clone_from(&title);
        }
        let app_id_changed = data.app_id != app_id;
        if app_id_changed {
            data.app_id.clone_from(&app_id);
        }
        let states_changed = data.states != states;
        if states_changed {
            data.states = states;
        }
        let output_changed = data.output != new_output;
        if output_changed {
            data.output = new_output;
        }

        if !(title_changed || app_id_changed || states_changed || output_changed) {
            return;
        }

        let state_bytes = states_to_bytes(&data.states);
        let output = data.output.clone();
        for (handle, entered) in data.handles.iter_mut() {
            if title_changed {
                handle.title(title.clone());
            }
            if app_id_changed {
                handle.app_id(app_id.clone());
            }
            if states_changed {
                handle.state(state_bytes.clone());
            }
            if output_changed {
                for wl_output in entered.drain(..) {
                    handle.output_leave(&wl_output);
                }
                if let (Some(output), Some(client)) = (&output, handle.client()) {
                    for wl_output in output.client_outputs(&client) {
                        handle.output_enter(&wl_output);
                        entered.push(wl_output);
                    }
                }
            }
            handle.done();
        }
    }

    /// Re-evaluate every tracked window after a focus change so the synthetic
    /// `activated` state moves with keyboard focus.
    pub(crate) fn refresh_all_wlr_toplevels(&mut self) {
        let surfaces: Vec<WlSurface> = self
            .foreign_toplevel_manager
            .toplevels
            .keys()
            .cloned()
            .collect();
        for surface in surfaces {
            if let Some(window) = self.window_for_toplevel_surface(&surface) {
                self.refresh_wlr_toplevel(&window);
            }
        }
    }

    /// Tell every handle the toplevel is gone and stop tracking it. The handles
    /// become inert; the client destroys them at its leisure.
    pub(crate) fn unregister_wlr_toplevel(&mut self, window: &Window) {
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else {
            return;
        };
        if let Some(data) = self.foreign_toplevel_manager.toplevels.remove(&surface) {
            for handle in data.handles.keys() {
                handle.closed();
            }
        }
    }

    /// The wlr state array for `window`: committed maximized/fullscreen plus the
    /// `activated` bit when it is the keyboard-focused window.
    fn wlr_state_vec(&self, window: &Window) -> Vec<u32> {
        let (maximized, fullscreen) = window
            .toplevel()
            .map(|toplevel| {
                let states = toplevel.current_state().states;
                (
                    states.contains(xdg_toplevel::State::Maximized),
                    states.contains(xdg_toplevel::State::Fullscreen),
                )
            })
            .unwrap_or((false, false));
        let activated = self.focused_ipc.is_some() && window_id(window) == self.focused_ipc;
        state_flags(maximized, fullscreen, activated)
    }

    /// A new manager bound: announce every tracked window to it, then keep the
    /// instance so future windows reach it too.
    fn wlr_manager_bound(
        &mut self,
        display: &DisplayHandle,
        manager: ZwlrForeignToplevelManagerV1,
    ) {
        for (surface, data) in self.foreign_toplevel_manager.toplevels.iter_mut() {
            data.add_handle(display, &manager, surface);
        }
        self.foreign_toplevel_manager.managers.push(manager);
    }

    /// A manager stopped or disconnected: stop announcing windows to it. Its
    /// handles are dropped from each window's set as they are destroyed.
    fn wlr_manager_gone(&mut self, manager: &ZwlrForeignToplevelManagerV1) {
        self.foreign_toplevel_manager
            .managers
            .retain(|existing| existing != manager);
    }

    /// A handle was destroyed by its client: drop it from its window's set.
    fn wlr_handle_gone(&mut self, surface: &WlSurface, handle: &ZwlrForeignToplevelHandleV1) {
        if let Some(data) = self.foreign_toplevel_manager.toplevels.get_mut(surface) {
            data.handles.remove(handle);
        }
    }

    fn wlr_activate(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_toplevel_surface(surface) else {
            return;
        };
        // The window may live on an inactive canvas; switch to it first so
        // `activate` reveals the window instead of polluting the active canvas's
        // focus stack with a surface mapped elsewhere.
        let target = self.wm.canvases().find_map(|canvas| {
            canvas
                .space()
                .elements()
                .any(|element| element == &window)
                .then(|| (canvas.id(), canvas.name().to_owned()))
        });
        if let Some((id, name)) = target
            && !self.wm.is_active(id)
        {
            self.switch_workspace(WorkspaceRef::Name(name));
        }
        self.focus_window(window);
    }

    fn wlr_close(&mut self, surface: &WlSurface) {
        if let Some(toplevel) = self.toplevel_for_surface(surface) {
            toplevel.send_close();
        }
    }

    fn wlr_set_fullscreen(&mut self, surface: &WlSurface, output: Option<WlOutput>) {
        if let Some(toplevel) = self.toplevel_for_surface(surface) {
            self.fullscreen_request(toplevel, output);
        }
    }

    fn wlr_unset_fullscreen(&mut self, surface: &WlSurface) {
        if let Some(toplevel) = self.toplevel_for_surface(surface) {
            self.unfullscreen_request(toplevel);
        }
    }

    fn wlr_set_maximized(&mut self, surface: &WlSurface) {
        if let Some(toplevel) = self.toplevel_for_surface(surface) {
            self.maximize_request(toplevel);
        }
    }

    fn wlr_unset_maximized(&mut self, surface: &WlSurface) {
        if let Some(toplevel) = self.toplevel_for_surface(surface) {
            self.unmaximize_request(toplevel);
        }
    }

    /// Resolve a tracked surface to its owning toplevel, ignoring inert handles
    /// whose window has already been unmapped.
    fn toplevel_for_surface(
        &self,
        surface: &WlSurface,
    ) -> Option<smithay::wayland::shell::xdg::ToplevelSurface> {
        self.window_for_toplevel_surface(surface)
            .and_then(|window| window.toplevel().cloned())
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for SayukiState {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        state.wlr_manager_bound(handle, manager);
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for SayukiState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            resource.finished();
            state.wlr_manager_gone(resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrForeignToplevelManagerV1,
        _data: &(),
    ) {
        // Covers a sudden disconnect where `stop` was never sent.
        state.wlr_manager_gone(resource);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, WlSurface> for SayukiState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        surface: &WlSurface,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request;
        match request {
            Request::Activate { .. } => state.wlr_activate(surface),
            Request::Close => state.wlr_close(surface),
            Request::SetFullscreen { output } => state.wlr_set_fullscreen(surface, output),
            Request::UnsetFullscreen => state.wlr_unset_fullscreen(surface),
            Request::SetMaximized => state.wlr_set_maximized(surface),
            Request::UnsetMaximized => state.wlr_unset_maximized(surface),
            // No minimize concept on the canvas; the rectangle is only a hint.
            Request::SetMinimized | Request::UnsetMinimized | Request::SetRectangle { .. } => {}
            Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ZwlrForeignToplevelHandleV1,
        surface: &WlSurface,
    ) {
        state.wlr_handle_gone(surface, resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wlr(state: zwlr_foreign_toplevel_handle_v1::State) -> u32 {
        state as u32
    }

    #[test]
    fn state_flags_includes_only_set_states() {
        assert!(state_flags(false, false, false).is_empty());
        assert_eq!(
            state_flags(true, false, false),
            vec![wlr(zwlr_foreign_toplevel_handle_v1::State::Maximized)]
        );
        assert_eq!(
            state_flags(true, true, true),
            vec![
                wlr(zwlr_foreign_toplevel_handle_v1::State::Maximized),
                wlr(zwlr_foreign_toplevel_handle_v1::State::Fullscreen),
                wlr(zwlr_foreign_toplevel_handle_v1::State::Activated),
            ]
        );
    }

    #[test]
    fn activated_is_independent_of_other_states() {
        assert_eq!(
            state_flags(false, false, true),
            vec![wlr(zwlr_foreign_toplevel_handle_v1::State::Activated)]
        );
    }

    #[test]
    fn states_encode_as_native_endian_u32_array() {
        let bytes = states_to_bytes(&[
            wlr(zwlr_foreign_toplevel_handle_v1::State::Maximized),
            wlr(zwlr_foreign_toplevel_handle_v1::State::Activated),
        ]);
        let mut expected = Vec::new();
        expected.extend_from_slice(
            &wlr(zwlr_foreign_toplevel_handle_v1::State::Maximized).to_ne_bytes(),
        );
        expected.extend_from_slice(
            &wlr(zwlr_foreign_toplevel_handle_v1::State::Activated).to_ne_bytes(),
        );
        assert_eq!(bytes, expected);
        assert_eq!(bytes.len(), 8);
    }
}
