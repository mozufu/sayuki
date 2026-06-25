//! Window-management policy for Sayuki's project canvas model: a viewport over
//! an unbounded canvas, oriented around projects.
//!
//! A **canvas is a `Space`**. [`WindowManager`] owns the canvases and the active
//! one; switching canvases moves the cameras (outputs), not the furniture
//! (windows), so window positions persist for free and switching is
//! `O(outputs)`. Only the active canvas has its outputs mapped into its `Space`;
//! switching unmaps from the old canvas (emitting `wl_surface.leave`) and maps
//! into the new one (emitting `enter`).
//!
//! The mechanism here knows nothing about discovery or trust; the compositor
//! resolves a [`project::ProjectContext`] and hands it to a [`Canvas`].

pub mod focus;
pub mod pin;
pub mod project;
pub mod snap;
pub mod swap;
pub mod viewport;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use smithay::{
    desktop::{Space, Window},
    output::Output,
    utils::{Logical, Point, Rectangle},
};

use crate::{
    focus::CycleDirection,
    pin::Pinned,
    project::{CanvasHooks, ProjectContext, WindowRule},
    snap::SnapConfig,
    viewport::Viewport,
};

/// The concrete window element a canvas holds. Aliased so the single coupling
/// point to Smithay's `desktop::Window` is named once: a future genericization
/// over `SpaceElement` is a mechanical replacement of this alias.
pub type WmWindow = Window;

/// Whether pan/zoom gestures act on one viewport or all of them together.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PanCouple {
    /// A gesture acts on the focused output's viewport only (per-monitor
    /// cameras). The default.
    #[default]
    Independent,
    /// All viewports pan/zoom together, preserving relative offsets (one sheet
    /// of glass).
    Linked,
}

/// A reference to a workspace/canvas by 1-based numeric index or project name
/// (generalized in 5b; `workspace = 1` and `workspace = "sayuki"` both work).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceRef {
    Index(u8),
    Name(String),
}

/// Stable identifier for a [`Canvas`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanvasId(u32);

impl CanvasId {
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// One canvas: an unbounded plane (its own `Space`) plus the per-output cameras
/// looking at it, a focus stack, and pinned HUD windows.
pub struct Canvas {
    id: CanvasId,
    name: String,
    space: Space<Window>,
    /// Per output name: where this canvas looks and how far it is zoomed.
    viewports: HashMap<String, Viewport>,
    /// Per output name: the viewport saved before entering the overview, so the
    /// toggle can restore it. Presence means "in overview".
    overview: HashMap<String, Viewport>,
    /// Outputs currently showing the persistent minimap.
    minimap: HashSet<String>,
    /// MRU focus stack; `last()` is focused.
    focus: Vec<Window>,
    /// Windows stuck to an output's viewport (HUDs).
    pinned: Vec<Pinned>,
    /// Project working directory (5b); `None` for a bare canvas.
    working_dir: Option<PathBuf>,
    /// Small env overlay applied to spawns (5b) — NOT a replacement for the
    /// inherited environment, which direnv owns.
    env: Vec<(String, String)>,
    /// Lifecycle hooks (5b).
    hooks: CanvasHooks,
    /// Map-time window-routing rules owned by this project canvas (5b).
    rules: Vec<WindowRule>,
    /// Declarative apps launched once on first activation (5b).
    apps: Vec<String>,
    /// One-shot guard so `on_init`/`apps` run only on the first activation.
    initialized: bool,
}

impl Canvas {
    fn new(id: CanvasId, name: String) -> Self {
        Self {
            id,
            name,
            space: Space::default(),
            viewports: HashMap::new(),
            overview: HashMap::new(),
            minimap: HashSet::new(),
            focus: Vec::new(),
            pinned: Vec::new(),
            working_dir: None,
            env: Vec::new(),
            hooks: CanvasHooks::default(),
            rules: Vec::new(),
            apps: Vec::new(),
            initialized: false,
        }
    }

    /// A canvas carrying a resolved project context (5b).
    fn with_context(id: CanvasId, name: String, context: ProjectContext) -> Self {
        Self {
            working_dir: context.working_dir,
            env: context.env,
            hooks: context.hooks,
            rules: context.rules,
            apps: context.apps,
            ..Self::new(id, name)
        }
    }

    /// Apply a project context to an existing canvas (e.g. when a `[[project]]`
    /// name collides with the default canvas) rather than dropping it.
    fn set_context(&mut self, context: ProjectContext) {
        self.working_dir = context.working_dir;
        self.env = context.env;
        self.hooks = context.hooks;
        self.rules = context.rules;
        self.apps = context.apps;
    }

    pub fn id(&self) -> CanvasId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn working_dir(&self) -> Option<&Path> {
        self.working_dir.as_deref()
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    /// Commands to spawn when this canvas becomes active: on the first
    /// activation the declarative `apps` then `on_init` (guarded by
    /// `initialized`), then `on_enter` every time. Each entry is an argv to run
    /// in the canvas's spawn context.
    pub fn enter(&mut self) -> Vec<Vec<String>> {
        let mut commands = Vec::new();
        if !self.initialized {
            self.initialized = true;
            for app in &self.apps {
                let argv: Vec<String> = app.split_whitespace().map(str::to_owned).collect();
                if !argv.is_empty() {
                    commands.push(argv);
                }
            }
            if let Some(on_init) = &self.hooks.on_init {
                commands.push(on_init.to_args());
            }
        }
        if let Some(on_enter) = &self.hooks.on_enter {
            commands.push(on_enter.to_args());
        }
        commands
    }

    /// Commands to spawn when this canvas is left (`on_leave`).
    pub fn leave(&self) -> Vec<Vec<String>> {
        self.hooks
            .on_leave
            .as_ref()
            .map(|hook| vec![hook.to_args()])
            .unwrap_or_default()
    }

    pub fn space(&self) -> &Space<Window> {
        &self.space
    }

    pub fn space_mut(&mut self) -> &mut Space<Window> {
        &mut self.space
    }

    /// The viewport for `output_name`, or a native viewport at the origin when
    /// the canvas has never been shown there.
    pub fn viewport(&self, output_name: &str) -> Viewport {
        self.viewports
            .get(output_name)
            .copied()
            .unwrap_or_else(|| Viewport::new(Point::from((0, 0))))
    }

    /// The viewport for `output`, seeding a contiguous default (the output's
    /// physical location) the first time the canvas is shown there.
    fn ensure_viewport(&mut self, output: &Output) -> Viewport {
        *self
            .viewports
            .entry(output.name())
            .or_insert_with(|| Viewport::new(output.current_location()))
    }

    pub fn set_viewport(&mut self, output_name: &str, viewport: Viewport) {
        self.viewports.insert(output_name.to_owned(), viewport);
    }

    /// Toggle the overview for `output_name`. Entering saves the current
    /// viewport and applies `fit`; leaving restores the saved viewport and
    /// returns it so the caller can re-map the output.
    pub fn toggle_overview(&mut self, output_name: &str, fit: Viewport) -> Viewport {
        if let Some(saved) = self.overview.remove(output_name) {
            self.viewports.insert(output_name.to_owned(), saved);
            saved
        } else {
            let current = self.viewport(output_name);
            self.overview.insert(output_name.to_owned(), current);
            self.viewports.insert(output_name.to_owned(), fit);
            fit
        }
    }

    pub fn minimap_enabled(&self, output_name: &str) -> bool {
        self.minimap.contains(output_name)
    }

    pub fn toggle_minimap(&mut self, output_name: &str) {
        if !self.minimap.remove(output_name) {
            self.minimap.insert(output_name.to_owned());
        }
    }

    /// Focus `window`, moving it to the MRU tail.
    pub fn focus(&mut self, window: Window) {
        focus::focus(&mut self.focus, window);
    }

    /// Remove `window` from this canvas's focus stack and pin list. Returns
    /// `true` when it was the focused window.
    pub fn remove(&mut self, window: &Window) -> bool {
        self.pinned.retain(|pinned| &pinned.window != window);
        focus::remove(&mut self.focus, window)
    }

    pub fn focused(&self) -> Option<&Window> {
        self.focus.last()
    }

    /// Rotate focus and return the newly focused window.
    pub fn cycle(&mut self, direction: CycleDirection) -> Option<&Window> {
        focus::cycle(&mut self.focus, direction);
        self.focus.last()
    }

    /// The MRU neighbour to swap with: `Prev` is the previously focused window,
    /// `Next` the least recently used, both relative to the focused tail.
    pub fn mru_neighbor(&self, next: bool) -> Option<Window> {
        if self.focus.len() < 2 {
            return None;
        }
        let window = if next {
            self.focus.first()
        } else {
            self.focus.get(self.focus.len() - 2)
        };
        window.cloned()
    }

    pub fn pinned(&self) -> &[Pinned] {
        &self.pinned
    }

    pub fn is_pinned(&self, window: &Window) -> bool {
        self.pinned.iter().any(|pinned| &pinned.window == window)
    }

    pub fn add_pin(&mut self, pinned: Pinned) {
        self.pinned
            .retain(|existing| existing.window != pinned.window);
        self.pinned.push(pinned);
    }

    pub fn remove_pin(&mut self, window: &Window) -> bool {
        let before = self.pinned.len();
        self.pinned.retain(|pinned| &pinned.window != window);
        self.pinned.len() != before
    }
}

/// Owns every canvas and the active selection plus shared WM policy config.
pub struct WindowManager {
    canvases: Vec<Canvas>,
    active: CanvasId,
    next_id: u32,
    pan_couple: PanCouple,
    snap: SnapConfig,
}

impl WindowManager {
    /// Create the manager with a single canvas named `"1"`, seeding contiguous
    /// viewports for `outputs` and mapping them into the active canvas.
    pub fn new(
        outputs: &[Output],
        pan_couple: PanCouple,
        snap: SnapConfig,
        projects: Vec<(String, ProjectContext)>,
    ) -> Self {
        let id = CanvasId(0);
        let mut canvas = Canvas::new(id, "1".to_owned());
        for output in outputs {
            let viewport = canvas.ensure_viewport(output);
            canvas.space.map_output(output, viewport.loc);
        }

        let mut canvases = vec![canvas];
        let mut next_id = 1;
        for (name, context) in projects {
            if let Some(existing) = canvases.iter_mut().find(|canvas| canvas.name == name) {
                existing.set_context(context);
                continue;
            }
            let project_id = CanvasId(next_id);
            next_id += 1;
            canvases.push(Canvas::with_context(project_id, name, context));
        }

        Self {
            canvases,
            active: id,
            next_id,
            pan_couple,
            snap,
        }
    }

    pub fn pan_couple(&self) -> PanCouple {
        self.pan_couple
    }

    pub fn snap(&self) -> &SnapConfig {
        &self.snap
    }

    pub fn set_pan_couple(&mut self, couple: PanCouple) {
        self.pan_couple = couple;
    }

    pub fn set_snap(&mut self, snap: SnapConfig) {
        self.snap = snap;
    }

    fn index_of(&self, id: CanvasId) -> usize {
        self.canvases
            .iter()
            .position(|canvas| canvas.id == id)
            .expect("canvas ids are never removed")
    }

    pub fn active(&self) -> &Canvas {
        let index = self.index_of(self.active);
        &self.canvases[index]
    }

    pub fn active_mut(&mut self) -> &mut Canvas {
        let index = self.index_of(self.active);
        &mut self.canvases[index]
    }

    pub fn active_space(&self) -> &Space<Window> {
        self.active().space()
    }

    pub fn active_space_mut(&mut self) -> &mut Space<Window> {
        self.active_mut().space_mut()
    }

    pub fn is_active(&self, id: CanvasId) -> bool {
        self.active == id
    }

    pub fn canvases(&self) -> impl Iterator<Item = &Canvas> {
        self.canvases.iter()
    }

    /// Mutable access to a specific canvas (e.g. the target of a window move).
    pub fn canvas_mut(&mut self, id: CanvasId) -> &mut Canvas {
        let index = self.index_of(id);
        &mut self.canvases[index]
    }

    /// The bounding rectangle of the active canvas's per-output **visible
    /// regions** (`[viewport.loc, output_size / zoom]`) — the region the pointer
    /// is clamped to so it can travel across every monitor and the gaps between
    /// them, and so it can reach the whole zoomed-out canvas in the overview.
    pub fn viewport_union(&self) -> Option<Rectangle<i32, Logical>> {
        let canvas = self.active();
        let space = canvas.space();
        viewport::bounding_rect(space.outputs().filter_map(|output| {
            let geometry = space.output_geometry(output)?;
            let viewport = canvas.viewport(&output.name());
            Some(viewport::visible_region(&viewport, geometry.size))
        }))
    }

    /// Resolve a workspace reference to a canvas id, creating a bare canvas when
    /// no project or numeric canvas of that name exists yet.
    pub fn canvas_for(&mut self, reference: WorkspaceRef) -> CanvasId {
        let name = match reference {
            WorkspaceRef::Index(index) => index.to_string(),
            WorkspaceRef::Name(name) => name,
        };
        if let Some(canvas) = self.canvases.iter().find(|canvas| canvas.name == name) {
            return canvas.id;
        }

        let id = CanvasId(self.next_id);
        self.next_id += 1;
        self.canvases.push(Canvas::new(id, name));
        id
    }

    /// The project canvas whose rules pin a window with `app_id`/`title`, if any.
    pub fn pin_target(&self, app_id: Option<&str>, title: Option<&str>) -> Option<CanvasId> {
        self.canvases.iter().find_map(|canvas| {
            canvas
                .rules
                .iter()
                .any(|rule| rule.pin && rule.matches(app_id, title))
                .then_some(canvas.id)
        })
    }

    /// Switch the active canvas to `target`. Unmaps `outputs` from the old
    /// canvas (emitting leave), maps them into the target (emitting enter,
    /// seeding default viewports), refreshes both spaces, and returns the
    /// window the caller should focus (the target's MRU tail).
    pub fn switch_to(&mut self, target: CanvasId, outputs: &[Output]) -> Option<Window> {
        if target == self.active {
            return self.active().focused().cloned();
        }

        let old_index = self.index_of(self.active);
        {
            let old = &mut self.canvases[old_index];
            for output in outputs {
                old.space.unmap_output(output);
            }
            old.space.refresh();
        }

        self.active = target;

        let new_index = self.index_of(target);
        let new = &mut self.canvases[new_index];
        for output in outputs {
            let viewport = new.ensure_viewport(output);
            new.space.map_output(output, viewport.loc);
        }
        new.space.refresh();
        new.focused().cloned()
    }

    /// Map a freshly hot-plugged output into the active canvas at a contiguous
    /// default viewport. Inactive canvases pick it up lazily on switch.
    pub fn map_output_active(&mut self, output: &Output) {
        let canvas = self.active_mut();
        let viewport = canvas.ensure_viewport(output);
        canvas.space.map_output(output, viewport.loc);
    }

    /// Unmap a removed output from every canvas's space and drop its viewport.
    /// Windows visible only via that output keep their canvas coordinates.
    pub fn unmap_output_all(&mut self, output: &Output) {
        let name = output.name();
        for canvas in &mut self.canvases {
            canvas.space.unmap_output(output);
            canvas.viewports.remove(&name);
            canvas.overview.remove(&name);
            canvas.minimap.remove(&name);
        }
    }

    /// Find a window across every canvas matching `predicate`. Window counts are
    /// tiny, so a linear scan is fine.
    pub fn window_for<F: Fn(&Window) -> bool>(&self, predicate: F) -> Option<Window> {
        self.canvases
            .iter()
            .flat_map(|canvas| canvas.space.elements())
            .find(|window| predicate(window))
            .cloned()
    }

    /// Remove a window from whichever canvas holds it: unmap from the space and
    /// drop it from the focus stack and pin list. Returns `true` when the window
    /// was the active canvas's focused window, so the caller re-focuses the new
    /// tail (deterministic focus-after-close).
    pub fn remove_window(&mut self, window: &Window) -> bool {
        let active = self.active;
        let mut removed_active_focus = false;
        for canvas in &mut self.canvases {
            canvas.space.unmap_elem(window);
            let was_focused = canvas.remove(window);
            if canvas.id == active && was_focused {
                removed_active_focus = true;
            }
        }
        removed_active_focus
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{ProjectContext, WindowRule};

    fn project_with_rule() -> ProjectContext {
        ProjectContext {
            rules: vec![WindowRule {
                app_id: Some("firefox".to_owned()),
                title: None,
                pin: true,
            }],
            ..ProjectContext::default()
        }
    }

    #[test]
    fn pin_target_routes_matching_window_to_project_canvas() {
        let manager = WindowManager::new(
            &[],
            PanCouple::Independent,
            SnapConfig::default(),
            vec![("sayuki".to_owned(), project_with_rule())],
        );

        // A matching window resolves to the project canvas, which is not the
        // active default canvas "1".
        let target = manager
            .pin_target(Some("firefox"), None)
            .expect("firefox matches the project rule");
        assert!(!manager.is_active(target));
        // Non-matching windows are not routed (they stay on the active canvas).
        assert_eq!(manager.pin_target(Some("ghostty"), None), None);
    }

    #[test]
    fn canvas_for_resolves_index_and_name() {
        let mut manager = WindowManager::new(
            &[],
            PanCouple::Independent,
            SnapConfig::default(),
            vec![("sayuki".to_owned(), project_with_rule())],
        );

        // The pre-created project canvas is reachable by name and is the same
        // canvas the rule pins to.
        let by_name = manager.canvas_for(WorkspaceRef::Name("sayuki".to_owned()));
        assert_eq!(manager.pin_target(Some("firefox"), None), Some(by_name));
        // A numeric reference resolves to (or creates) a distinct bare canvas.
        let by_index = manager.canvas_for(WorkspaceRef::Index(2));
        assert_ne!(by_index, by_name);
    }
}
