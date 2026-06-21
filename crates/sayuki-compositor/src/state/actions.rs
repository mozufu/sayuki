//! Window-manager policy actions driven by keybindings (and, later, IPC).
//!
//! These methods orchestrate the canvas/viewport model on top of the core
//! mechanism in `state.rs`: focus, canvas switching, pan/zoom/overview/minimap,
//! pinning, snap and swap. They live in a child module so the core file stays
//! focused; as a descendant of `state` they still reach the private
//! `SayukiState` fields and helpers directly.

use std::path::Path;

use smithay::{
    desktop::Window,
    output::Output,
    utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER},
};

use super::{SayukiState, set_window_size};
use crate::{
    render,
    wm::{
        PanCouple, WorkspaceRef,
        focus::CycleDirection,
        pin::{self, Pinned, ViewportAnchor},
        snap,
        swap::{self, Direction, SwapTarget},
        viewport,
    },
};

impl SayukiState {
    pub(super) fn focus_window_at(&mut self, location: Point<f64, Logical>) {
        let Some(window) = self
            .space()
            .element_under(location)
            .map(|(window, _)| window.clone())
        else {
            let keyboard = self.keyboard.clone();
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
            return;
        };

        self.focus_window(window);
    }

    /// Focus `window`: move it to the MRU tail, raise it, reveal it if it is
    /// off-screen, and hand it keyboard focus.
    pub(super) fn focus_window(&mut self, window: Window) {
        self.wm.active_mut().focus(window.clone());
        self.apply_focus(Some(window));
    }

    /// Apply focus effects (raise + reveal + keyboard) for an already-selected
    /// window, or clear focus when `None`.
    pub(super) fn apply_focus(&mut self, window: Option<Window>) {
        let keyboard = self.keyboard.clone();
        let serial = SERIAL_COUNTER.next_serial();
        match window {
            Some(window) => {
                self.space_mut().raise_element(&window, true);
                self.reveal_window(&window);
                self.send_pending_window_configures();
                let surface = window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface().clone());
                keyboard.set_focus(self, surface, serial);
            }
            None => keyboard.set_focus(self, None, serial),
        }
    }

    /// Reveal-on-focus: pan the focused window's output the minimum distance to
    /// bring it fully into view.
    fn reveal_window(&mut self, window: &Window) {
        let Some(rect) = self.space().element_geometry(window) else {
            return;
        };
        let Some(output) = self.output_for_rect(rect) else {
            return;
        };
        let Some(geometry) = self.space().output_geometry(&output) else {
            return;
        };

        let viewport = self.wm.active().viewport(&output.name());
        let revealed = viewport::reveal_pan(&viewport, geometry.size, rect);
        let delta = revealed - viewport.loc;
        if delta.x == 0 && delta.y == 0 {
            return;
        }

        match self.wm.pan_couple() {
            PanCouple::Linked => self.pan_viewport(delta),
            PanCouple::Independent => {
                let mut viewport = viewport;
                viewport.loc = revealed;
                self.wm.active_mut().set_viewport(&output.name(), viewport);
                self.apply_viewport(&output);
            }
        }
    }

    pub(super) fn switch_workspace(&mut self, reference: WorkspaceRef) {
        let target = self.wm.canvas_for(reference);
        if self.wm.is_active(target) {
            return;
        }

        // Capture the outgoing canvas's on_leave hook in its own context.
        let leave = self.wm.active().leave();
        let leave_dir = self.wm.active().working_dir().map(Path::to_owned);
        let leave_env = self.wm.active().env().to_vec();

        let outputs = self.collect_outputs();
        let focus = self.wm.switch_to(target, &outputs);
        self.run_commands(leave, leave_dir.as_deref(), &leave_env);

        // Run the incoming canvas's enter hooks (one-shot on_init/apps first).
        let enter = self.wm.active_mut().enter();
        let enter_dir = self.wm.active().working_dir().map(Path::to_owned);
        let enter_env = self.wm.active().env().to_vec();
        self.run_commands(enter, enter_dir.as_deref(), &enter_env);

        self.apply_focus(focus);
        self.send_pending_window_configures();
    }

    pub(super) fn move_focused_to_workspace(&mut self, reference: WorkspaceRef) {
        let Some(window) = self.wm.active().focused().cloned() else {
            return;
        };
        let target = self.wm.canvas_for(reference);
        if self.wm.is_active(target) {
            return;
        }

        self.space_mut().unmap_elem(&window);
        self.wm.active_mut().remove(&window);

        let region = self.primary_output_geometry();
        let location = viewport::placement_location(region, self.next_window_index);
        self.next_window_index = self.next_window_index.wrapping_add(1);
        let canvas = self.wm.canvas_mut(target);
        canvas
            .space_mut()
            .map_element(window.clone(), location, false);
        canvas.focus(window);

        let focus = self.wm.active().focused().cloned();
        self.apply_focus(focus);
        self.send_pending_window_configures();
    }

    pub(super) fn pan_viewport(&mut self, delta: Point<i32, Logical>) {
        for output in self.pan_zoom_outputs() {
            let name = output.name();
            let mut viewport = self.wm.active().viewport(&name);
            viewport.loc = viewport::clamp_pan(viewport.loc + delta);
            self.wm.active_mut().set_viewport(&name, viewport);
            self.apply_viewport(&output);
        }
    }

    pub(super) fn zoom_viewport(&mut self, factor: f64) {
        match self.wm.pan_couple() {
            PanCouple::Independent => {
                if let Some(output) = self.focused_output() {
                    self.zoom_output(&output, factor);
                }
            }
            PanCouple::Linked => {
                // Derive one zoom and one loc delta from the focused output and
                // apply both to every viewport, preserving the relative offsets
                // between outputs (one sheet of glass).
                let Some(focused) = self.focused_output() else {
                    return;
                };
                let Some(geometry) = self.space().output_geometry(&focused) else {
                    return;
                };
                let current = self.wm.active().viewport(&focused.name());
                let zoomed = viewport::zoom_about_center(&current, geometry.size, factor);
                let delta = zoomed.loc - current.loc;
                for output in self.collect_outputs() {
                    let name = output.name();
                    let mut viewport = self.wm.active().viewport(&name);
                    viewport.zoom = zoomed.zoom;
                    viewport.loc = viewport::clamp_pan(viewport.loc + delta);
                    self.wm.active_mut().set_viewport(&name, viewport);
                    self.apply_viewport(&output);
                }
            }
        }
    }

    fn zoom_output(&mut self, output: &Output, factor: f64) {
        let Some(geometry) = self.space().output_geometry(output) else {
            return;
        };
        let name = output.name();
        let viewport = self.wm.active().viewport(&name);
        let zoomed = viewport::zoom_about_center(&viewport, geometry.size, factor);
        self.wm.active_mut().set_viewport(&name, zoomed);
        self.apply_viewport(output);
    }

    pub(super) fn toggle_overview(&mut self) {
        let Some(output) = self.focused_output() else {
            return;
        };
        let Some(geometry) = self.space().output_geometry(&output) else {
            return;
        };
        let name = output.name();
        let current = self.wm.active().viewport(&name);
        let fit = self
            .canvas_bounds()
            .map(|bounds| viewport::fit_viewport(geometry.size, bounds, render::OVERVIEW_MARGIN))
            .unwrap_or(current);
        self.wm.active_mut().toggle_overview(&name, fit);
        self.apply_viewport(&output);
    }

    pub(super) fn toggle_minimap(&mut self) {
        let Some(output) = self.focused_output() else {
            return;
        };
        self.wm.active_mut().toggle_minimap(&output.name());
    }

    pub(super) fn toggle_pin_focused(&mut self) {
        let Some(window) = self.wm.active().focused().cloned() else {
            return;
        };

        if self.wm.active().is_pinned(&window) {
            self.wm.active_mut().remove_pin(&window);
            return;
        }

        let Some(rect) = self.space().element_geometry(&window) else {
            return;
        };
        let Some(output) = self.output_for_rect(rect) else {
            return;
        };
        let Some(geometry) = self.space().output_geometry(&output) else {
            return;
        };

        let anchor = pin::capture_anchor(geometry, rect);
        let location = pin::pinned_location(geometry, rect.size, &anchor);
        self.space_mut().map_element(window.clone(), location, true);
        self.wm.active_mut().add_pin(Pinned {
            window,
            output: output.name(),
            anchor,
        });
    }

    /// Re-map an output at its viewport's location and recompute the canvas
    /// locations of pinned windows so they stay fixed on screen after a
    /// pan/zoom.
    fn apply_viewport(&mut self, output: &Output) {
        let location = self.wm.active().viewport(&output.name()).loc;
        self.space_mut().map_output(output, location);
        self.reposition_pinned_for_output(output);
    }

    fn reposition_pinned_for_output(&mut self, output: &Output) {
        let name = output.name();
        let Some(geometry) = self.space().output_geometry(output) else {
            return;
        };
        let pins: Vec<(Window, ViewportAnchor)> = self
            .wm
            .active()
            .pinned()
            .iter()
            .filter(|pinned| pinned.output == name)
            .map(|pinned| (pinned.window.clone(), pinned.anchor))
            .collect();

        for (window, anchor) in pins {
            if !window.alive() {
                continue;
            }
            let size = self
                .space()
                .element_geometry(&window)
                .map(|rect| rect.size)
                .unwrap_or_else(|| window.geometry().size);
            let location = pin::pinned_location(geometry, size, &anchor);
            self.space_mut().map_element(window, location, false);
        }
    }

    pub(super) fn swap_focused(&mut self, target: SwapTarget) {
        let Some(focused) = self.wm.active().focused().cloned() else {
            return;
        };
        let other = match target {
            SwapTarget::Direction(direction) => self.nearest_window(&focused, direction),
            SwapTarget::Next => self.wm.active().mru_neighbor(true),
            SwapTarget::Prev => self.wm.active().mru_neighbor(false),
        };
        let Some(other) = other else {
            return;
        };
        if other == focused {
            return;
        }
        self.exchange_windows(&focused, &other);
    }

    fn nearest_window(&self, focused: &Window, direction: Direction) -> Option<Window> {
        let space = self.space();
        let focused_rect = space.element_geometry(focused)?;
        let windows: Vec<(Window, Rectangle<i32, Logical>)> = space
            .elements()
            .filter(|window| *window != focused)
            .filter_map(|window| {
                space
                    .element_geometry(window)
                    .map(|rect| (window.clone(), rect))
            })
            .collect();
        let candidates: Vec<(usize, Rectangle<i32, Logical>)> = windows
            .iter()
            .enumerate()
            .map(|(index, (_, rect))| (index, *rect))
            .collect();
        let index = swap::nearest_in_direction(focused_rect, &candidates, direction)?;
        windows.into_iter().nth(index).map(|(window, _)| window)
    }

    fn exchange_windows(&mut self, first: &Window, second: &Window) {
        let space = self.space();
        let Some(first_location) = space.element_location(first) else {
            return;
        };
        let Some(second_location) = space.element_location(second) else {
            return;
        };
        let first_rect = Rectangle::new(first_location, first.geometry().size);
        let second_rect = Rectangle::new(second_location, second.geometry().size);
        let (new_first, new_second) = swap::exchange(first_rect, second_rect);

        set_window_size(first, new_first.size);
        set_window_size(second, new_second.size);
        self.space_mut()
            .map_element(first.clone(), new_first.loc, true);
        self.space_mut()
            .map_element(second.clone(), new_second.loc, false);
        self.send_pending_window_configures();
    }

    /// Snap an interactive move's proposed top-left to nearby window/viewport
    /// edges and the optional grid. Adjusts only the drop point; storage stays
    /// free coordinates.
    pub(crate) fn snap_move(
        &self,
        window: &Window,
        proposed: Point<i32, Logical>,
    ) -> Point<i32, Logical> {
        let space = self.space();
        let dragged = Rectangle::new(proposed, window.geometry().size);
        let window_edges: Vec<Rectangle<i32, Logical>> = space
            .elements()
            .filter(|element| *element != window)
            .filter_map(|element| {
                space
                    .element_location(element)
                    .map(|location| Rectangle::new(location, element.geometry().size))
            })
            .collect();
        let viewport_edges: Vec<Rectangle<i32, Logical>> = space
            .outputs()
            .filter_map(|output| space.output_geometry(output))
            .collect();
        snap::snap_location(dragged, &window_edges, &viewport_edges, self.wm.snap())
    }

    /// Edge-push auto-pan: while a window is dragged past a viewport edge, pan
    /// that output's viewport so the canvas scrolls under the drag.
    pub(crate) fn edge_push_pan(&mut self, pointer: Point<f64, Logical>) {
        const EDGE_MARGIN: f64 = 8.0;
        const EDGE_STEP: i32 = 24;

        let Some(output) = self.space().output_under(pointer).next().cloned() else {
            return;
        };
        let Some(geometry) = self.space().output_geometry(&output) else {
            return;
        };
        let geometry = geometry.to_f64();

        let mut delta = Point::<i32, Logical>::from((0, 0));
        if pointer.x - geometry.loc.x < EDGE_MARGIN {
            delta.x = -EDGE_STEP;
        } else if (geometry.loc.x + geometry.size.w) - pointer.x < EDGE_MARGIN {
            delta.x = EDGE_STEP;
        }
        if pointer.y - geometry.loc.y < EDGE_MARGIN {
            delta.y = -EDGE_STEP;
        } else if (geometry.loc.y + geometry.size.h) - pointer.y < EDGE_MARGIN {
            delta.y = EDGE_STEP;
        }
        if delta.x == 0 && delta.y == 0 {
            return;
        }

        let name = output.name();
        let mut viewport = self.wm.active().viewport(&name);
        viewport.loc = viewport::clamp_pan(viewport.loc + delta);
        self.wm.active_mut().set_viewport(&name, viewport);
        self.apply_viewport(&output);
    }

    /// Drop-onto-window swap: if a move grab is released with the pointer over a
    /// different window, exchange the two windows' rectangles.
    pub(crate) fn handle_move_drop(&mut self, window: &Window) {
        let pointer = self.pointer_location;
        let space = self.space();
        let target = space
            .elements()
            .rev()
            .filter(|candidate| *candidate != window)
            .find(|candidate| {
                space
                    .element_geometry(candidate)
                    .map(|rect| rect.to_f64().contains(pointer))
                    .unwrap_or(false)
            })
            .cloned();
        if let Some(target) = target {
            self.exchange_windows(window, &target);
        }
    }

    pub(super) fn cycle_focus(&mut self, direction: CycleDirection) {
        let window = self.wm.active_mut().cycle(direction).cloned();
        self.apply_focus(window);
    }

    fn pan_zoom_outputs(&self) -> Vec<Output> {
        match self.wm.pan_couple() {
            PanCouple::Linked => self.collect_outputs(),
            PanCouple::Independent => self.focused_output().into_iter().collect(),
        }
    }
}
