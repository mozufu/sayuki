use std::{
    error::Error,
    io::{self, ErrorKind},
    time::{Duration, Instant},
};

use calloop::LoopHandle;
use smithay::{
    backend::{
        drm::{DrmEvent, DrmEventMetadata as EventMetadata},
        input::{
            AbsolutePositionEvent, Axis, ButtonState, Device, Event as InputBackendEvent,
            InputBackend, InputEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
            PointerMotionEvent,
        },
        libinput::LibinputInputBackend,
        renderer::{
            Color32F, Frame, Renderer,
            element::{Kind, surface::render_elements_from_surface_tree},
            utils::{draw_render_elements, with_renderer_surface_state},
        },
        session::Event as SessionEvent,
        udev::UdevEvent,
        winit::WinitEvent,
    },
    desktop::{PopupManager, Space, Window, WindowSurfaceType, utils::send_frames_surface_tree},
    input::{
        Seat, SeatState,
        keyboard::KeyboardHandle,
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, CursorImageSurfaceData, Focus,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerHandle,
        },
    },
    output::Output,
    reexports::wayland_server::{
        Display, DisplayHandle, backend::GlobalId, protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Size, Transform},
    wayland::{
        compositor::{CompositorState, with_states},
        selection::data_device::DataDeviceState,
        shell::xdg::{ToplevelSurface, XdgShellState},
        shm::ShmState,
    },
};
use tracing::{debug, info};

use crate::{
    backend::BackendState,
    config::SayukiConfig,
    grabs::{PointerMoveSurfaceGrab, PointerResizeSurfaceGrab, ResizeEdge},
    input::{actions::CompositorAction, keybindings::KeybindingRegistry, spawn::ActionRunner},
};

const BACKGROUND: Color32F = Color32F::new(0.07, 0.08, 0.11, 1.0);
const WINDOW_STAGGER: i32 = 32;
const WINDOW_STAGGER_STEPS: i32 = 10;

pub(crate) struct SayukiState {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) space: Space<Window>,
    pub(crate) popups: PopupManager,
    pub(crate) windows: Vec<Window>,
    action_runner: ActionRunner,
    keybindings: KeybindingRegistry,

    display_handle: DisplayHandle,
    backend: BackendState,
    pending_output_global_removals: Vec<(GlobalId, Instant)>,
    _seat: Seat<Self>,
    keyboard: KeyboardHandle<Self>,
    pointer: PointerHandle<Self>,

    pointer_location: Point<f64, Logical>,
    cursor_image: CursorImageStatus,
    next_window_index: i32,
    start_time: Instant,
    pub(crate) running: bool,
}

impl SayukiState {
    pub(crate) fn new(
        display: &Display<Self>,
        config: SayukiConfig,
        backend: BackendState,
    ) -> Result<Self, Box<dyn Error>> {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");
        let keyboard = seat.add_keyboard(
            config.keyboard.xkb_config(),
            config.keyboard.repeat_delay,
            config.keyboard.repeat_rate,
        )?;
        let pointer = seat.add_pointer();
        let keybindings = KeybindingRegistry::from_configs(&config.keybindings)
            .map_err(|message| io::Error::new(ErrorKind::InvalidData, message))?;

        let mut space = Space::default();
        backend.for_each_output(|output| {
            space.map_output(output, output.current_location());
        });

        Ok(Self {
            compositor_state,
            xdg_shell_state,
            shm_state,
            data_device_state,
            seat_state,
            space,
            popups: PopupManager::default(),
            windows: Vec::new(),
            action_runner: ActionRunner::default(),
            keybindings,
            display_handle,
            backend,
            pending_output_global_removals: Vec::new(),
            _seat: seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            cursor_image: CursorImageStatus::default_named(),
            next_window_index: 0,
            start_time: Instant::now(),
            running: true,
        })
    }

    pub(crate) fn set_wayland_display(&mut self, wayland_display: String) {
        self.action_runner.set_wayland_display(wayland_display);
    }

    pub(crate) fn set_cursor_image(&mut self, image: CursorImageStatus) {
        self.cursor_image = image;
    }

    pub(crate) fn handle_winit_event(&mut self, event: WinitEvent) {
        match event {
            WinitEvent::CloseRequested => {
                info!("close requested");
                self.running = false;
            }
            WinitEvent::Focus(focused) => {
                debug!(focused, "nested window focus changed");
            }
            WinitEvent::Resized { size, scale_factor } => {
                debug!(?size, scale_factor, "nested window resized");
                self.configure_nested_output(size);
            }
            WinitEvent::Input(event) => self.handle_input(event),
            WinitEvent::Redraw => {}
        }
    }

    pub(crate) fn handle_input<B>(&mut self, event: InputEvent<B>)
    where
        B: InputBackend,
    {
        match event {
            InputEvent::Keyboard { event } => self.forward_keyboard_event::<B>(event),
            InputEvent::PointerMotion { event } => self.forward_pointer_relative_motion::<B>(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.forward_pointer_absolute_motion::<B>(event);
            }
            InputEvent::PointerButton { event } => self.forward_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.forward_pointer_axis::<B>(event),
            InputEvent::DeviceAdded { device } => {
                debug!(name = device.name(), "input device added")
            }
            InputEvent::DeviceRemoved { device } => {
                debug!(name = device.name(), "input device removed");
            }
            _ => {}
        }
    }

    fn forward_keyboard_event<B>(&mut self, event: B::KeyboardKeyEvent)
    where
        B: InputBackend,
    {
        let keycode = event.key_code();
        let key_state = event.state();
        let keyboard = self.keyboard.clone();
        let action = keyboard.input::<CompositorAction, _>(
            self,
            keycode,
            key_state,
            SERIAL_COUNTER.next_serial(),
            event.time_msec(),
            move |state, modifiers, handle| {
                state
                    .keybindings
                    .filter_key(keycode, key_state, modifiers, handle.modified_sym())
            },
        );
        if let Some(action) = action {
            self.run_action(action);
        }
    }

    fn run_action(&mut self, action: CompositorAction) {
        match action {
            CompositorAction::None => {}
            CompositorAction::Quit => {
                info!("quit action requested");
                self.running = false;
            }
            CompositorAction::Spawn(command) => self.action_runner.spawn(&command),
            CompositorAction::BeginMove => self.begin_move_focused_window(),
            CompositorAction::BeginResize(edges) => self.begin_resize_focused_window(edges),
            CompositorAction::SwitchWorkspace(workspace) => {
                debug!(workspace, "workspace switching is not implemented yet");
            }
        }
    }

    fn forward_pointer_relative_motion<B>(&mut self, event: B::PointerMotionEvent)
    where
        B: InputBackend,
    {
        let output_geometry = self.primary_output_geometry();
        let location =
            clamp_pointer_location(self.pointer_location + event.delta(), output_geometry);
        self.pointer_location = location;
        self.forward_pointer_motion_to_clients(location, event.time_msec());
    }

    fn forward_pointer_absolute_motion<B>(&mut self, event: B::PointerMotionAbsoluteEvent)
    where
        B: InputBackend,
    {
        let output_geometry = self.primary_output_geometry();
        let transformed_location =
            output_geometry.loc.to_f64() + event.position_transformed(output_geometry.size);
        let location = clamp_pointer_location(transformed_location, output_geometry);
        self.pointer_location = location;
        self.forward_pointer_motion_to_clients(location, event.time_msec());
    }

    fn forward_pointer_motion_to_clients(&mut self, location: Point<f64, Logical>, time: u32) {
        let pointer = self.pointer.clone();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    fn forward_pointer_button<B>(&mut self, event: B::PointerButtonEvent)
    where
        B: InputBackend,
    {
        let serial = SERIAL_COUNTER.next_serial();
        if event.state() == ButtonState::Pressed {
            self.focus_window_at(self.pointer_location, serial);
        }

        let pointer = self.pointer.clone();
        pointer.button(
            self,
            &ButtonEvent {
                serial,
                time: event.time_msec(),
                button: event.button_code(),
                state: event.state(),
            },
        );
        pointer.frame(self);
    }

    fn forward_pointer_axis<B>(&mut self, event: B::PointerAxisEvent)
    where
        B: InputBackend,
    {
        let mut frame = AxisFrame::new(event.time_msec()).source(event.source());

        for axis in [Axis::Horizontal, Axis::Vertical] {
            if let Some(amount) = event.amount(axis) {
                frame = frame.value(axis, amount);
            }
            if let Some(amount_v120) = event.amount_v120(axis) {
                frame = frame.v120(axis, amount_v120 as i32);
            }
            frame = frame.relative_direction(axis, event.relative_direction(axis));
        }

        let pointer = self.pointer.clone();
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    fn configure_nested_output(&mut self, size: Size<i32, Physical>) {
        let BackendState::Nested(backend) = &mut self.backend else {
            debug!("nested output configure ignored for non-nested backend");
            return;
        };

        backend.configure_output(size);
        let output = backend.output().clone();
        self.space.map_output(&output, output.current_location());
    }

    pub(crate) fn primary_output_geometry(&self) -> Rectangle<i32, Logical> {
        self.backend
            .primary_output()
            .and_then(|output| self.space.output_geometry(output))
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (800, 600).into()))
    }

    pub(crate) fn add_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.windows.push(window);
    }

    pub(crate) fn remove_toplevel(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window_for_toplevel_surface(surface) {
            self.space.unmap_elem(&window);
            self.windows.retain(|known_window| known_window != &window);
        }
    }

    pub(crate) fn place_window(&mut self, window: Window, activate: bool) {
        let output_geometry = self.primary_output_geometry();

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.bounds = Some(output_geometry.size);
            });
        }

        let step = self.next_window_index.rem_euclid(WINDOW_STAGGER_STEPS);
        self.next_window_index = self.next_window_index.wrapping_add(1);
        let offset = WINDOW_STAGGER * step;
        let location = output_geometry.loc + Point::<i32, Logical>::from((offset, offset));

        self.space.map_element(window, location, activate);
        self.send_pending_window_configures();
    }

    pub(crate) fn handle_surface_commit(&mut self, surface: &WlSurface) {
        self.windows.retain(Window::alive);

        if let Some(window) = self.window_for_toplevel_surface(surface) {
            window.on_commit();

            if surface_has_buffer(surface) {
                if self.space.element_location(&window).is_none() {
                    self.place_window(window, true);
                }
            } else {
                self.space.unmap_elem(&window);
            }
        } else if let Some(window) = self.window_for_surface(surface) {
            window.on_commit();
        }
    }

    pub(crate) fn ensure_initial_configure(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_toplevel_surface(surface) else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };

        if toplevel.is_initial_configure_sent() {
            return;
        }

        let bounds = self.primary_output_geometry().size;
        toplevel.with_pending_state(|state| {
            state.bounds = Some(bounds);
        });
        toplevel.send_configure();
    }

    pub(crate) fn window_for_toplevel_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.windows
            .iter()
            .find(|window| {
                window
                    .toplevel()
                    .map(|toplevel| toplevel.wl_surface() == surface)
                    .unwrap_or(false)
            })
            .cloned()
    }

    pub(crate) fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.windows
            .iter()
            .find(|window| {
                let mut found = false;
                window.with_surfaces(|window_surface, _| {
                    found |= window_surface == surface;
                });
                found
            })
            .cloned()
    }

    pub(crate) fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(location)
            .and_then(|(window, window_location)| {
                window
                    .surface_under(location - window_location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_location)| {
                        (surface, (window_location + surface_location).to_f64())
                    })
            })
    }

    pub(crate) fn refresh_space(&mut self) {
        self.space.refresh();
        self.action_runner.reap_children();

        let now = Instant::now();
        let mut index = 0;
        while index < self.pending_output_global_removals.len() {
            if now >= self.pending_output_global_removals[index].1 {
                let (global_id, _) = self.pending_output_global_removals.remove(index);
                self.display_handle.remove_global::<Self>(global_id);
            } else {
                index += 1;
            }
        }
    }

    pub(crate) fn handle_libinput_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        let should_forward =
            matches!(&self.backend, BackendState::Udev(backend) if backend.is_active());
        if should_forward {
            self.handle_input(event);
        } else {
            debug!("libinput event ignored while native backend is inactive");
        }
    }

    pub(crate) fn handle_session_event(
        &mut self,
        event: SessionEvent,
    ) -> Result<(), Box<dyn Error>> {
        let BackendState::Udev(backend) = &mut self.backend else {
            debug!("session event ignored for non-udev backend");
            return Ok(());
        };

        backend.handle_session_event(
            event,
            &self.display_handle,
            &mut self.space,
            &mut self.pending_output_global_removals,
        )
    }

    pub(crate) fn handle_udev_event(
        &mut self,
        event: UdevEvent,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<Self>,
    ) -> Result<(), Box<dyn Error>> {
        let BackendState::Udev(backend) = &mut self.backend else {
            debug!("udev event ignored for non-udev backend");
            return Ok(());
        };

        backend.handle_udev_event(
            event,
            display_handle,
            loop_handle,
            &mut self.space,
            &mut self.pending_output_global_removals,
        )
    }

    pub(crate) fn handle_drm_event(
        &mut self,
        device_id: u64,
        event: DrmEvent,
        metadata: Option<EventMetadata>,
        loop_handle: &LoopHandle<Self>,
    ) -> Result<(), Box<dyn Error>> {
        let output = {
            let BackendState::Udev(backend) = &mut self.backend else {
                debug!("DRM event ignored for non-udev backend");
                return Ok(());
            };

            backend.handle_drm_event(
                device_id,
                event,
                metadata,
                &self.display_handle,
                loop_handle,
                &mut self.space,
                &mut self.pending_output_global_removals,
            )?
        };

        if let Some(output) = output {
            self.send_frame_callbacks_for_output(
                &output,
                Duration::from_millis(u64::from(self.frame_time())),
            );
        }

        Ok(())
    }

    fn begin_move_focused_window(&mut self) {
        let Some(window) = self.focused_window() else {
            debug!("move action ignored because no window is focused");
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            debug!("move action ignored because the focused window is not mapped");
            return;
        };

        let start_data = self.pointer_grab_start_data();
        let pointer = self.pointer.clone();
        pointer.set_grab(
            self,
            PointerMoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            },
            SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
    }

    fn begin_resize_focused_window(&mut self, edges: ResizeEdge) {
        if edges.is_empty() {
            debug!("resize action ignored because no resize edge was selected");
            return;
        }

        let Some(window) = self.focused_window() else {
            debug!("resize action ignored because no window is focused");
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            debug!("resize action ignored because the focused window is not mapped");
            return;
        };

        let mut initial_window_size = window.geometry().size;
        if initial_window_size.w <= 0 || initial_window_size.h <= 0 {
            initial_window_size = (100, 100).into();
        }

        let start_data = self.pointer_grab_start_data();
        let pointer = self.pointer.clone();
        pointer.set_grab(
            self,
            PointerResizeSurfaceGrab {
                start_data,
                window,
                edges,
                initial_window_location,
                initial_window_size,
                last_window_size: initial_window_size,
            },
            SERIAL_COUNTER.next_serial(),
            Focus::Clear,
        );
    }

    fn focused_window(&self) -> Option<Window> {
        self.keyboard
            .current_focus()
            .and_then(|surface| self.window_for_surface(&surface))
    }

    fn pointer_grab_start_data(&self) -> PointerGrabStartData<Self> {
        PointerGrabStartData {
            focus: self.surface_under(self.pointer_location),
            button: 0,
            location: self.pointer_location,
        }
    }

    pub(crate) fn render(&mut self) -> Result<(), Box<dyn Error>> {
        match &mut self.backend {
            BackendState::Nested(backend) => {
                let size = backend.window_size();
                if size.w == 0 || size.h == 0 {
                    return Ok(());
                }

                let damage = Rectangle::from_size(size);
                let logical_damage = Rectangle::from_size(size.to_logical(1));
                {
                    let (renderer, mut framebuffer) = backend.graphics_mut().bind()?;
                    let mut elements =
                        self.space
                            .render_elements_for_region(renderer, &logical_damage, 1.0, 1.0);
                    if let CursorImageStatus::Surface(surface) = &self.cursor_image {
                        let hotspot = cursor_hotspot(surface).to_f64();
                        let cursor_location: Point<i32, Physical> =
                            (self.pointer_location - hotspot).to_physical_precise_round(1.0);
                        elements.extend(render_elements_from_surface_tree(
                            renderer,
                            surface,
                            cursor_location,
                            1.0,
                            1.0,
                            Kind::Unspecified,
                        ));
                    }
                    let mut frame =
                        renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
                    frame.clear(BACKGROUND, &[damage])?;
                    draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;
                    let _sync_point = frame.finish()?;
                }
                backend.graphics_mut().submit(Some(&[damage]))?;
                self.send_frame_callbacks_for_all_outputs();
            }
            BackendState::Udev(backend) => {
                backend.render(
                    &self.space,
                    &self.cursor_image,
                    self.pointer_location,
                    BACKGROUND,
                )?;
            }
        }

        Ok(())
    }

    pub(crate) fn frame_time(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    fn focus_window_at(&mut self, location: Point<f64, Logical>, serial: smithay::utils::Serial) {
        let keyboard = self.keyboard.clone();
        let Some(window) = self
            .space
            .element_under(location)
            .map(|(window, _)| window.clone())
        else {
            keyboard.set_focus(self, None, serial);
            return;
        };

        let focus = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone());
        self.space.raise_element(&window, true);
        self.send_pending_window_configures();
        keyboard.set_focus(self, focus, serial);
    }

    fn send_pending_window_configures(&self) {
        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }

    fn send_frame_callbacks_for_output(&self, output: &Output, time: Duration) {
        for window in self.space.elements() {
            let outputs = self.space.outputs_for_element(window);
            if outputs.is_empty() {
                if self.backend.primary_output() == Some(output) {
                    window.send_frame(output, time, Some(Duration::ZERO), |_, _| None);
                }
                continue;
            }

            for candidate in outputs {
                if &candidate == output {
                    window.send_frame(&candidate, time, Some(Duration::ZERO), |_, _| None);
                }
            }
        }

        if let Some(cursor_output) = self.cursor_frame_output()
            && cursor_output == *output
            && let CursorImageStatus::Surface(surface) = &self.cursor_image
        {
            send_frames_surface_tree(surface, output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
    }

    fn send_frame_callbacks_for_all_outputs(&self) {
        let time = Duration::from_millis(u64::from(self.frame_time()));
        for window in self.space.elements() {
            let outputs = self.space.outputs_for_element(window);
            if outputs.is_empty() {
                if let Some(output) = self.backend.primary_output() {
                    window.send_frame(output, time, Some(Duration::ZERO), |_, _| None);
                }
                continue;
            }

            for output in outputs {
                window.send_frame(&output, time, Some(Duration::ZERO), |_, _| None);
            }
        }

        if let Some(output) = self.cursor_frame_output()
            && let CursorImageStatus::Surface(surface) = &self.cursor_image
        {
            send_frames_surface_tree(surface, &output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
    }

    fn cursor_frame_output(&self) -> Option<Output> {
        let mut cursor_output = None;
        self.backend.for_each_output(|output| {
            if cursor_output.is_none()
                && self
                    .space
                    .output_geometry(output)
                    .map(|geometry| geometry.to_f64().contains(self.pointer_location))
                    .unwrap_or(false)
            {
                cursor_output = Some(output.clone());
            }
        });

        cursor_output.or_else(|| self.backend.primary_output().cloned())
    }
}

pub(crate) fn cursor_hotspot(surface: &WlSurface) -> Point<i32, Logical> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .and_then(|attributes| attributes.lock().ok().map(|attributes| attributes.hotspot))
            .unwrap_or_else(|| (0, 0).into())
    })
}

fn clamp_pointer_location(
    location: Point<f64, Logical>,
    bounds: Rectangle<i32, Logical>,
) -> Point<f64, Logical> {
    let min_x = f64::from(bounds.loc.x);
    let min_y = f64::from(bounds.loc.y);
    let max_x = f64::from(bounds.loc.x + bounds.size.w.max(1) - 1);
    let max_y = f64::from(bounds.loc.y + bounds.size.h.max(1) - 1);

    (
        location.x.clamp(min_x, max_x),
        location.y.clamp(min_y, max_y),
    )
        .into()
}

fn surface_has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use smithay::utils::{Logical, Point, Rectangle};

    use super::clamp_pointer_location;

    #[test]
    fn relative_pointer_motion_clamps_to_output() {
        let bounds = Rectangle::<i32, Logical>::new((10, 20).into(), (100, 50).into());
        let start: Point<f64, Logical> = (30.0, 40.0).into();
        let negative_delta: Point<f64, Logical> = (-50.0, -50.0).into();
        let positive_delta: Point<f64, Logical> = (500.0, 500.0).into();

        assert_eq!(
            clamp_pointer_location(start + negative_delta, bounds),
            (10.0, 20.0).into()
        );
        assert_eq!(
            clamp_pointer_location(start + positive_delta, bounds),
            (109.0, 69.0).into()
        );
    }
}
