//! Window manager policy: a viewport over an unbounded canvas, oriented around
//! projects.
//!
//! A **canvas is a `Space`**. [`WindowManager`] owns the canvases and the active
//! one; switching canvases moves the cameras (outputs), not the furniture
//! (windows), so window positions persist for free and switching is
//! `O(outputs)`. Only the active canvas has its outputs mapped into its `Space`;
//! switching unmaps from the old canvas (emitting `wl_surface.leave`) and maps
//! into the new one (emitting `enter`).
//!
//! The mechanism here knows nothing about projects; milestone 5b appends the
//! project context to [`Canvas`].

pub(crate) mod focus;
pub(crate) mod pin;
pub(crate) mod snap;
pub(crate) mod swap;
pub(crate) mod viewport;

use std::collections::{HashMap, HashSet};

use smithay::{
    desktop::{Space, Window},
    output::Output,
    utils::{Logical, Point, Rectangle},
};

use self::{focus::CycleDirection, pin::Pinned, snap::SnapConfig, viewport::Viewport};

/// Whether pan/zoom gestures act on one viewport or all of them together.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PanCouple {
    /// A gesture acts on the focused output's viewport only (per-monitor
    /// cameras). The default.
    #[default]
    Independent,
    /// All viewports pan/zoom together, preserving relative offsets (one sheet
    /// of glass).
    Linked,
}

/// Stable identifier for a [`Canvas`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CanvasId(u32);

/// One canvas: an unbounded plane (its own `Space`) plus the per-output cameras
/// looking at it, a focus stack, and pinned HUD windows.
pub(crate) struct Canvas {
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
        }
    }

    pub(crate) fn space(&self) -> &Space<Window> {
        &self.space
    }

    pub(crate) fn space_mut(&mut self) -> &mut Space<Window> {
        &mut self.space
    }

    /// The viewport for `output_name`, or a native viewport at the origin when
    /// the canvas has never been shown there.
    pub(crate) fn viewport(&self, output_name: &str) -> Viewport {
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

    pub(crate) fn set_viewport(&mut self, output_name: &str, viewport: Viewport) {
        self.viewports.insert(output_name.to_owned(), viewport);
    }

    /// Toggle the overview for `output_name`. Entering saves the current
    /// viewport and applies `fit`; leaving restores the saved viewport and
    /// returns it so the caller can re-map the output.
    pub(crate) fn toggle_overview(&mut self, output_name: &str, fit: Viewport) -> Viewport {
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

    pub(crate) fn minimap_enabled(&self, output_name: &str) -> bool {
        self.minimap.contains(output_name)
    }

    pub(crate) fn toggle_minimap(&mut self, output_name: &str) {
        if !self.minimap.remove(output_name) {
            self.minimap.insert(output_name.to_owned());
        }
    }

    /// Focus `window`, moving it to the MRU tail.
    pub(crate) fn focus(&mut self, window: Window) {
        focus::focus(&mut self.focus, window);
    }

    /// Remove `window` from this canvas's focus stack and pin list. Returns
    /// `true` when it was the focused window.
    pub(crate) fn remove(&mut self, window: &Window) -> bool {
        self.pinned.retain(|pinned| &pinned.window != window);
        focus::remove(&mut self.focus, window)
    }

    pub(crate) fn focused(&self) -> Option<&Window> {
        self.focus.last()
    }

    /// Rotate focus and return the newly focused window.
    pub(crate) fn cycle(&mut self, direction: CycleDirection) -> Option<&Window> {
        focus::cycle(&mut self.focus, direction);
        self.focus.last()
    }

    /// The MRU neighbour to swap with: `Prev` is the previously focused window,
    /// `Next` the least recently used, both relative to the focused tail.
    pub(crate) fn mru_neighbor(&self, next: bool) -> Option<Window> {
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

    pub(crate) fn pinned(&self) -> &[Pinned] {
        &self.pinned
    }

    pub(crate) fn is_pinned(&self, window: &Window) -> bool {
        self.pinned.iter().any(|pinned| &pinned.window == window)
    }

    pub(crate) fn add_pin(&mut self, pinned: Pinned) {
        self.pinned
            .retain(|existing| existing.window != pinned.window);
        self.pinned.push(pinned);
    }

    pub(crate) fn remove_pin(&mut self, window: &Window) -> bool {
        let before = self.pinned.len();
        self.pinned.retain(|pinned| &pinned.window != window);
        self.pinned.len() != before
    }
}

/// Owns every canvas and the active selection plus shared WM policy config.
pub(crate) struct WindowManager {
    canvases: Vec<Canvas>,
    active: CanvasId,
    next_id: u32,
    pan_couple: PanCouple,
    snap: SnapConfig,
}

impl WindowManager {
    /// Create the manager with a single canvas named `"1"`, seeding contiguous
    /// viewports for `outputs` and mapping them into the active canvas.
    pub(crate) fn new(outputs: &[Output], pan_couple: PanCouple, snap: SnapConfig) -> Self {
        let id = CanvasId(0);
        let mut canvas = Canvas::new(id, "1".to_owned());
        for output in outputs {
            let viewport = canvas.ensure_viewport(output);
            canvas.space.map_output(output, viewport.loc);
        }

        Self {
            canvases: vec![canvas],
            active: id,
            next_id: 1,
            pan_couple,
            snap,
        }
    }

    pub(crate) fn pan_couple(&self) -> PanCouple {
        self.pan_couple
    }

    pub(crate) fn snap(&self) -> &SnapConfig {
        &self.snap
    }

    fn index_of(&self, id: CanvasId) -> usize {
        self.canvases
            .iter()
            .position(|canvas| canvas.id == id)
            .expect("canvas ids are never removed")
    }

    pub(crate) fn active(&self) -> &Canvas {
        let index = self.index_of(self.active);
        &self.canvases[index]
    }

    pub(crate) fn active_mut(&mut self) -> &mut Canvas {
        let index = self.index_of(self.active);
        &mut self.canvases[index]
    }

    pub(crate) fn active_space(&self) -> &Space<Window> {
        self.active().space()
    }

    pub(crate) fn active_space_mut(&mut self) -> &mut Space<Window> {
        self.active_mut().space_mut()
    }

    pub(crate) fn is_active(&self, id: CanvasId) -> bool {
        self.active == id
    }

    /// Mutable access to a specific canvas (e.g. the target of a window move).
    pub(crate) fn canvas_mut(&mut self, id: CanvasId) -> &mut Canvas {
        let index = self.index_of(id);
        &mut self.canvases[index]
    }

    /// The bounding rectangle of the active canvas's per-output **visible
    /// regions** (`[viewport.loc, output_size / zoom]`) — the region the pointer
    /// is clamped to so it can travel across every monitor and the gaps between
    /// them, and so it can reach the whole zoomed-out canvas in the overview.
    pub(crate) fn viewport_union(&self) -> Option<Rectangle<i32, Logical>> {
        let canvas = self.active();
        let space = canvas.space();
        viewport::bounding_rect(space.outputs().filter_map(|output| {
            let geometry = space.output_geometry(output)?;
            let viewport = canvas.viewport(&output.name());
            Some(viewport::visible_region(&viewport, geometry.size))
        }))
    }

    /// Find the canvas named after `index` (1-based workspace number), creating
    /// it if necessary.
    pub(crate) fn canvas_for_index(&mut self, index: u8) -> CanvasId {
        let name = index.to_string();
        if let Some(canvas) = self.canvases.iter().find(|canvas| canvas.name == name) {
            return canvas.id;
        }

        let id = CanvasId(self.next_id);
        self.next_id += 1;
        self.canvases.push(Canvas::new(id, name));
        id
    }

    /// Switch the active canvas to `target`. Unmaps `outputs` from the old
    /// canvas (emitting leave), maps them into the target (emitting enter,
    /// seeding default viewports), refreshes both spaces, and returns the
    /// window the caller should focus (the target's MRU tail).
    pub(crate) fn switch_to(&mut self, target: CanvasId, outputs: &[Output]) -> Option<Window> {
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
    pub(crate) fn map_output_active(&mut self, output: &Output) {
        let canvas = self.active_mut();
        let viewport = canvas.ensure_viewport(output);
        canvas.space.map_output(output, viewport.loc);
    }

    /// Unmap a removed output from every canvas's space and drop its viewport.
    /// Windows visible only via that output keep their canvas coordinates.
    pub(crate) fn unmap_output_all(&mut self, output: &Output) {
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
    pub(crate) fn window_for<F: Fn(&Window) -> bool>(&self, predicate: F) -> Option<Window> {
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
    pub(crate) fn remove_window(&mut self, window: &Window) -> bool {
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
