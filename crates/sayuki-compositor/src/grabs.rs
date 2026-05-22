use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::state::SayukiState;

macro_rules! delegate_pointer_grab_methods {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            focus: Option<(WlSurface, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }

        fn axis(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            details: AxisFrame,
        ) {
            handle.axis(data, details);
        }

        fn frame(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
        ) {
            handle.frame(data);
        }

        fn gesture_swipe_begin(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GestureSwipeBeginEvent,
        ) {
            handle.gesture_swipe_begin(data, event);
        }

        fn gesture_swipe_update(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GestureSwipeUpdateEvent,
        ) {
            handle.gesture_swipe_update(data, event);
        }

        fn gesture_swipe_end(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GestureSwipeEndEvent,
        ) {
            handle.gesture_swipe_end(data, event);
        }

        fn gesture_pinch_begin(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GesturePinchBeginEvent,
        ) {
            handle.gesture_pinch_begin(data, event);
        }

        fn gesture_pinch_update(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GesturePinchUpdateEvent,
        ) {
            handle.gesture_pinch_update(data, event);
        }

        fn gesture_pinch_end(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GesturePinchEndEvent,
        ) {
            handle.gesture_pinch_end(data, event);
        }

        fn gesture_hold_begin(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GestureHoldBeginEvent,
        ) {
            handle.gesture_hold_begin(data, event);
        }

        fn gesture_hold_end(
            &mut self,
            data: &mut SayukiState,
            handle: &mut PointerInnerHandle<'_, SayukiState>,
            event: &GestureHoldEndEvent,
        ) {
            handle.gesture_hold_end(data, event);
        }
    };
}

pub(crate) struct PointerMoveSurfaceGrab {
    pub(crate) start_data: PointerGrabStartData<SayukiState>,
    pub(crate) window: Window,
    pub(crate) initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<SayukiState> for PointerMoveSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut SayukiState,
        handle: &mut PointerInnerHandle<'_, SayukiState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.space
            .map_element(self.window.clone(), new_location.to_i32_round(), true);
    }

    delegate_pointer_grab_methods!();

    fn button(
        &mut self,
        data: &mut SayukiState,
        handle: &mut PointerInnerHandle<'_, SayukiState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn start_data(&self) -> &PointerGrabStartData<SayukiState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut SayukiState) {}
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub(crate) struct ResizeEdge: u32 {
        const NONE = 0;
        const TOP = 1;
        const BOTTOM = 2;
        const LEFT = 4;
        const TOP_LEFT = Self::TOP.bits() | Self::LEFT.bits();
        const BOTTOM_LEFT = Self::BOTTOM.bits() | Self::LEFT.bits();
        const RIGHT = 8;
        const TOP_RIGHT = Self::TOP.bits() | Self::RIGHT.bits();
        const BOTTOM_RIGHT = Self::BOTTOM.bits() | Self::RIGHT.bits();
    }
}

impl From<xdg_toplevel::ResizeEdge> for ResizeEdge {
    fn from(edge: xdg_toplevel::ResizeEdge) -> Self {
        Self::from_bits_truncate(edge as u32)
    }
}

pub(crate) struct PointerResizeSurfaceGrab {
    pub(crate) start_data: PointerGrabStartData<SayukiState>,
    pub(crate) window: Window,
    pub(crate) edges: ResizeEdge,
    pub(crate) initial_window_location: Point<i32, Logical>,
    pub(crate) initial_window_size: Size<i32, Logical>,
    pub(crate) last_window_size: Size<i32, Logical>,
}

impl PointerResizeSurfaceGrab {
    fn resized_location(&self, size: Size<i32, Logical>) -> Point<i32, Logical> {
        let mut location = self.initial_window_location;

        if self.edges.intersects(ResizeEdge::LEFT) {
            location.x = self.initial_window_location.x + (self.initial_window_size.w - size.w);
        }
        if self.edges.intersects(ResizeEdge::TOP) {
            location.y = self.initial_window_location.y + (self.initial_window_size.h - size.h);
        }

        location
    }
}

impl PointerGrab<SayukiState> for PointerResizeSurfaceGrab {
    fn motion(
        &mut self,
        data: &mut SayukiState,
        handle: &mut PointerInnerHandle<'_, SayukiState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }

        let (mut dx, mut dy) = (event.location - self.start_data.location).into();
        let mut new_size = self.initial_window_size;

        if self.edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            if self.edges.intersects(ResizeEdge::LEFT) {
                dx = -dx;
            }
            new_size.w = (self.initial_window_size.w as f64 + dx).round() as i32;
        }

        if self.edges.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
            if self.edges.intersects(ResizeEdge::TOP) {
                dy = -dy;
            }
            new_size.h = (self.initial_window_size.h as f64 + dy).round() as i32;
        }

        self.last_window_size = clamp_window_size(&self.window, new_size);
        request_window_size(&self.window, self.last_window_size, true);

        if self.edges.intersects(ResizeEdge::TOP_LEFT) {
            data.space.map_element(
                self.window.clone(),
                self.resized_location(self.last_window_size),
                true,
            );
        }
    }

    delegate_pointer_grab_methods!();

    fn button(
        &mut self,
        data: &mut SayukiState,
        handle: &mut PointerInnerHandle<'_, SayukiState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if !handle.current_pressed().is_empty() {
            return;
        }

        handle.unset_grab(self, data, event.serial, event.time, true);

        if !self.window.alive() {
            return;
        }

        request_window_size(&self.window, self.last_window_size, false);
        if self.edges.intersects(ResizeEdge::TOP_LEFT) {
            data.space.map_element(
                self.window.clone(),
                self.resized_location(self.last_window_size),
                true,
            );
        }
    }

    fn start_data(&self) -> &PointerGrabStartData<SayukiState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut SayukiState) {}
}

fn clamp_window_size(window: &Window, size: Size<i32, Logical>) -> Size<i32, Logical> {
    let Some(surface) = window.toplevel().map(|toplevel| toplevel.wl_surface()) else {
        return size;
    };

    let (min_size, max_size) = with_states(surface, |states| {
        let mut guard = states.cached_state.get::<SurfaceCachedState>();
        let data = guard.current();
        (data.min_size, data.max_size)
    });

    let min_width = min_size.w.max(1);
    let min_height = min_size.h.max(1);
    let max_width = if max_size.w == 0 {
        i32::MAX
    } else {
        max_size.w
    };
    let max_height = if max_size.h == 0 {
        i32::MAX
    } else {
        max_size.h
    };

    (
        size.w.max(min_width).min(max_width),
        size.h.max(min_height).min(max_height),
    )
        .into()
}

fn request_window_size(window: &Window, size: Size<i32, Logical>, resizing: bool) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };

    toplevel.with_pending_state(|state| {
        if resizing {
            state.states.set(xdg_toplevel::State::Resizing);
        } else {
            state.states.unset(xdg_toplevel::State::Resizing);
        }
        state.size = Some(size);
    });
    toplevel.send_pending_configure();
}
