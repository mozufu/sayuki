use std::{
    error::Error,
    io::{self, ErrorKind},
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, ButtonState, Device, Event as InputBackendEvent,
            InputEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        },
        renderer::{
            Color32F, Frame, Renderer,
            element::{Kind, surface::render_elements_from_surface_tree},
            gles::GlesRenderer,
            utils::{draw_render_elements, with_renderer_surface_state},
        },
        winit::{WinitEvent, WinitGraphicsBackend, WinitInput},
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
    reexports::wayland_server::{Display, protocol::wl_surface::WlSurface},
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
    config::SayukiConfig,
    grabs::{PointerMoveSurfaceGrab, PointerResizeSurfaceGrab, ResizeEdge},
    input::{actions::CompositorAction, keybindings::KeybindingRegistry, spawn::ActionRunner},
    output::{configure_output, create_output},
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

    backend: WinitGraphicsBackend<GlesRenderer>,
    pub(crate) output: Output,
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
        backend: WinitGraphicsBackend<GlesRenderer>,
        config: SayukiConfig,
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

        let output = create_output(&display_handle, backend.window_size());
        let mut space = Space::default();
        space.map_output(&output, (0, 0));

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
            backend,
            output,
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
                self.configure_output(size);
            }
            WinitEvent::Input(event) => self.handle_input(event),
            WinitEvent::Redraw => {}
        }
    }

    fn handle_input(&mut self, event: InputEvent<WinitInput>) {
        match event {
            InputEvent::Keyboard { event } => self.forward_keyboard_event(event),
            InputEvent::PointerMotionAbsolute { event } => self.forward_pointer_motion(event),
            InputEvent::PointerButton { event } => self.forward_pointer_button(event),
            InputEvent::PointerAxis { event } => self.forward_pointer_axis(event),
            InputEvent::DeviceAdded { device } => {
                debug!(name = device.name(), "input device added")
            }
            InputEvent::DeviceRemoved { device } => {
                debug!(name = device.name(), "input device removed");
            }
            _ => {}
        }
    }

    fn forward_keyboard_event(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::KeyboardKeyEvent,
    ) {
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

    fn forward_pointer_motion(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerMotionAbsoluteEvent,
    ) {
        let logical_size = self.logical_output_size();
        let location = event.position_transformed(logical_size);
        self.pointer_location = location;

        let pointer = self.pointer.clone();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn forward_pointer_button(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerButtonEvent,
    ) {
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

    fn forward_pointer_axis(
        &mut self,
        event: <WinitInput as smithay::backend::input::InputBackend>::PointerAxisEvent,
    ) {
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

    fn configure_output(&mut self, size: Size<i32, Physical>) {
        configure_output(&self.output, size);
        self.space.map_output(&self.output, (0, 0));
    }

    fn logical_output_size(&self) -> Size<i32, Logical> {
        self.output
            .current_mode()
            .map(|mode| mode.size.to_logical(1))
            .unwrap_or_else(|| self.backend.window_size().to_logical(1))
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
        let output_geometry = self
            .space
            .output_geometry(&self.output)
            .unwrap_or_else(|| Rectangle::from_size(self.logical_output_size()));

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

        let bounds = self
            .space
            .output_geometry(&self.output)
            .map(|geometry| geometry.size)
            .unwrap_or_else(|| self.logical_output_size());
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
        let size = self.backend.window_size();
        if size.w == 0 || size.h == 0 {
            return Ok(());
        }

        let damage = Rectangle::from_size(size);
        let logical_damage = Rectangle::from_size(size.to_logical(1));
        {
            let (renderer, mut framebuffer) = self.backend.bind()?;
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
            let mut frame = renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
            frame.clear(BACKGROUND, &[damage])?;
            draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;
            let _sync_point = frame.finish()?;
        }
        self.backend.submit(Some(&[damage]))?;
        self.send_frame_callbacks();

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

    fn send_frame_callbacks(&self) {
        let time = Duration::from_millis(u64::from(self.frame_time()));
        for window in self.space.elements() {
            window.send_frame(&self.output, time, Some(Duration::ZERO), |_, _| None);
        }

        if let CursorImageStatus::Surface(surface) = &self.cursor_image {
            send_frames_surface_tree(surface, &self.output, time, Some(Duration::ZERO), |_, _| {
                Some(self.output.clone())
            });
        }
    }
}

fn cursor_hotspot(surface: &WlSurface) -> Point<i32, Logical> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<CursorImageSurfaceData>()
            .and_then(|attributes| attributes.lock().ok().map(|attributes| attributes.hotspot))
            .unwrap_or_else(|| (0, 0).into())
    })
}

fn surface_has_buffer(surface: &WlSurface) -> bool {
    with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false)
}
