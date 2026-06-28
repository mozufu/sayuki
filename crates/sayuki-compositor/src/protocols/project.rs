//! `zsayuki_project_v1`: per-surface project affinity.
//!
//! A trusted first-party client declares, before a toplevel maps, which project
//! canvas a surface belongs to and where it should sit in that canvas's
//! free-floating coordinate space. The compositor applies the declaration as the
//! surface maps (`SayukiState::consume_project_affinity` in `state/project.rs`)
//! instead of guessing affinity from app_id/pid after the fact.
//!
//! This module is the hand-written `GlobalDispatch`/`Dispatch` glue over the
//! generated bindings, mirroring `screencopy.rs`. The global is advertised only
//! to trusted clients via [`GlobalDispatch::can_view`] (milestone 9's
//! security-context trust gate): a sandboxed client can neither see nor bind it.

use sayuki_protocols::project::server::{
    zsayuki_project_manager_v1::{self, ZsayukiProjectManagerV1},
    zsayuki_project_surface_v1::{self, ZsayukiProjectSurfaceV1},
};
use smithay::{
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        backend::{ClientId, GlobalId},
        protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Point},
    wayland::compositor::with_states,
};

use crate::state::SayukiState;

/// Manager version advertised. v1 — additive evolution only.
const MANAGER_VERSION: u32 = 1;

/// Owns the `zsayuki_project_manager_v1` global for its lifetime.
pub(crate) struct ProjectManagerState {
    _global: GlobalId,
}

impl ProjectManagerState {
    pub(crate) fn new(display: &DisplayHandle) -> Self {
        let global =
            display.create_global::<SayukiState, ZsayukiProjectManagerV1, ()>(MANAGER_VERSION, ());
        Self { _global: global }
    }
}

/// Lifecycle of a per-surface affinity declaration. `set_*` requests are only
/// valid while [`Pending`](AffinityPhase::Pending); the first commit locks the
/// declaration and mapping applies it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AffinityPhase {
    /// Created; `set_*` requests are accepted.
    Pending,
    /// The window has mapped and the affinity has been applied.
    Mapped,
}

/// A pending per-surface project affinity, stored on [`SayukiState`] (one per
/// live `zsayuki_project_surface_v1`) and consumed when the surface first maps.
pub(crate) struct ProjectAffinity {
    /// The surface this affinity places — the key linking object to entry.
    pub(crate) surface: WlSurface,
    /// The protocol object, used to report the authoritative placement.
    pub(crate) object: ZsayukiProjectSurfaceV1,
    /// Requested project (canvas) name; `None` keeps the active project.
    pub(crate) project: Option<String>,
    /// Requested canvas coordinates; `None` keeps the staggered default.
    pub(crate) position: Option<Point<i32, Logical>>,
    /// Free-form rule hints (only `floating` is acted on; others are ignored).
    pub(crate) hints: Vec<(String, String)>,
    /// Where this declaration is in its lifecycle.
    pub(crate) phase: AffinityPhase,
}

impl ProjectAffinity {
    fn new(surface: WlSurface, object: ZsayukiProjectSurfaceV1) -> Self {
        Self {
            surface,
            object,
            project: None,
            position: None,
            hints: Vec::new(),
            phase: AffinityPhase::Pending,
        }
    }
}

impl GlobalDispatch<ZsayukiProjectManagerV1, ()> for SayukiState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZsayukiProjectManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    /// The global is advertised only to trusted first-party clients; a sandboxed
    /// (security-context) client can neither see nor bind it.
    fn can_view(client: Client, _global_data: &()) -> bool {
        crate::wayland::is_client_trusted(&client)
    }
}

impl Dispatch<ZsayukiProjectManagerV1, ()> for SayukiState {
    fn request(
        state: &mut Self,
        _client: &Client,
        manager: &ZsayukiProjectManagerV1,
        request: zsayuki_project_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zsayuki_project_manager_v1::Request::GetProjectSurface { id, surface } => {
                // One project-surface object per wl_surface.
                if state
                    .project_affinity
                    .iter()
                    .any(|affinity| affinity.surface == surface)
                {
                    let _ = data_init.init(id, ());
                    manager.post_error(
                        zsayuki_project_manager_v1::Error::AlreadyConstructed,
                        "wl_surface already has a project-surface object",
                    );
                    return;
                }
                let object = data_init.init(id, ());
                state
                    .project_affinity
                    .push(ProjectAffinity::new(surface, object));
            }
            zsayuki_project_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZsayukiProjectSurfaceV1, ()> for SayukiState {
    fn request(
        state: &mut Self,
        _client: &Client,
        object: &ZsayukiProjectSurfaceV1,
        request: zsayuki_project_surface_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zsayuki_project_surface_v1::Request::SetProject { name } => {
                set_pending(state, object, |affinity| affinity.project = Some(name));
            }
            zsayuki_project_surface_v1::Request::SetCanvasPosition { x, y } => {
                set_pending(state, object, |affinity| {
                    affinity.position = Some(Point::from((x, y)));
                });
            }
            zsayuki_project_surface_v1::Request::SetRuleHint { key, value } => {
                set_pending(state, object, |affinity| affinity.hints.push((key, value)));
            }
            zsayuki_project_surface_v1::Request::Destroy => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        object: &ZsayukiProjectSurfaceV1,
        _data: &(),
    ) {
        state
            .project_affinity
            .retain(|affinity| &affinity.object != object);
    }
}

/// Apply a `set_*` mutation to a still-pending affinity, or raise
/// `already_mapped` once the surface has had its first commit (affinity locks at
/// commit time, mirroring xdg role-state ordering). An unknown object (no entry)
/// is ignored — it has nothing to mutate.
fn set_pending(
    state: &mut SayukiState,
    object: &ZsayukiProjectSurfaceV1,
    apply: impl FnOnce(&mut ProjectAffinity),
) {
    let Some(index) = state
        .project_affinity
        .iter()
        .position(|affinity| &affinity.object == object)
    else {
        return;
    };
    if surface_has_committed(&state.project_affinity[index].surface) {
        object.post_error(
            zsayuki_project_surface_v1::Error::AlreadyMapped,
            "project affinity set after the surface's first commit",
        );
        return;
    }
    apply(&mut state.project_affinity[index]);
}

/// Marker inserted into a `wl_surface`'s data map on its first commit. Its
/// presence is the authoritative "this surface has committed" signal that locks
/// project affinity. It is recorded for every surface independently of whether a
/// `zsayuki_project_surface_v1` exists yet, so a declaration created *after* the
/// first commit is locked too (its setters post `already_mapped`).
struct AffinityCommitted;

/// Record that `surface` has committed at least once (idempotent). Called from
/// the surface commit handler for every commit.
pub(crate) fn mark_surface_committed(surface: &WlSurface) {
    with_states(surface, |states| {
        states.data_map.insert_if_missing(|| AffinityCommitted);
    });
}

/// Whether `surface` has had its first commit (see [`AffinityCommitted`]).
fn surface_has_committed(surface: &WlSurface) -> bool {
    with_states(surface, |states| {
        states.data_map.get::<AffinityCommitted>().is_some()
    })
}
