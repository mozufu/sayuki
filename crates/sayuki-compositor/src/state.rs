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
        renderer::{Color32F, Frame, Renderer, utils::draw_render_elements},
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
    utils::{Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Size, Transform},
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
    output::{self, OutputPolicy},
    render::{self, CursorRender},
    wm::{WindowManager, focus::CycleDirection, viewport},
};

mod actions;
mod project;

const BACKGROUND: Color32F = Color32F::new(0.07, 0.08, 0.11, 1.0);

pub(crate) struct SayukiState {
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) wm: WindowManager,
    pub(crate) popups: PopupManager,
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
    output_policies: Vec<OutputPolicy>,
    /// Windows awaiting one-shot window-rule routing once their client has set
    /// app_id/title (which arrive after the toplevel role is created).
    pending_rules: Vec<Window>,
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

        let output_policies = config.outputs;
        let mut outputs = Vec::new();
        backend.for_each_output(|output| outputs.push(output.clone()));
        for output in &outputs {
            output::apply_policy(output, &output_policies);
        }
        let projects = project::resolve_project_contexts(&config.projects);
        let wm = WindowManager::new(&outputs, config.pan_couple, config.snap, projects);

        Ok(Self {
            compositor_state,
            xdg_shell_state,
            shm_state,
            data_device_state,
            seat_state,
            wm,
            popups: PopupManager::default(),
            action_runner: ActionRunner::new(),
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
            output_policies,
            pending_rules: Vec::new(),
            start_time: Instant::now(),
            running: true,
        })
    }

    /// The active canvas's `Space` — the render/hit-test truth for the visible
    /// desktop.
    pub(crate) fn space(&self) -> &Space<Window> {
        self.wm.active_space()
    }

    pub(crate) fn space_mut(&mut self) -> &mut Space<Window> {
        self.wm.active_space_mut()
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
            CompositorAction::Spawn(command) => self.spawn_in_active(&command),
            CompositorAction::BeginMove => self.begin_move_focused_window(),
            CompositorAction::BeginResize(edges) => self.begin_resize_focused_window(edges),
            CompositorAction::SwitchWorkspace(workspace) => self.switch_workspace(workspace),
            CompositorAction::MoveToWorkspace(workspace) => {
                self.move_focused_to_workspace(workspace)
            }
            CompositorAction::PanViewport { dx, dy } => self.pan_viewport(Point::from((dx, dy))),
            CompositorAction::ZoomViewport(factor) => self.zoom_viewport(factor),
            CompositorAction::ToggleOverview => self.toggle_overview(),
            CompositorAction::ToggleMinimap => self.toggle_minimap(),
            CompositorAction::TogglePin => self.toggle_pin_focused(),
            CompositorAction::SwapWindow(target) => self.swap_focused(target),
            CompositorAction::FocusNext => self.cycle_focus(CycleDirection::Forward),
            CompositorAction::FocusPrev => self.cycle_focus(CycleDirection::Backward),
        }
    }

    fn forward_pointer_relative_motion<B>(&mut self, event: B::PointerMotionEvent)
    where
        B: InputBackend,
    {
        // A physical mouse delta should move the cursor the same screen distance
        // regardless of zoom, so divide the delta by the zoom under the pointer.
        let zoom = self.pointer_zoom();
        let delta = event.delta();
        let scaled = Point::from((delta.x / zoom, delta.y / zoom));
        let location = self.clamp_pointer(self.pointer_location + scaled);
        self.pointer_location = location;
        self.forward_pointer_motion_to_clients(location, event.time_msec());
    }

    fn forward_pointer_absolute_motion<B>(&mut self, event: B::PointerMotionAbsoluteEvent)
    where
        B: InputBackend,
    {
        let location = self
            .backend
            .primary_output()
            .cloned()
            .and_then(|output| {
                let geometry = self.space().output_geometry(&output)?;
                let viewport = self.wm.active().viewport(&output.name());
                let local = event.position_transformed(geometry.size);
                Some(viewport::to_canvas(&viewport, local))
            })
            .unwrap_or(self.pointer_location);
        let location = self.clamp_pointer(location);
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
            self.focus_window_at(self.pointer_location);
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
        output::apply_policy(&output, &self.output_policies);
        let location = self.wm.active().viewport(&output.name()).loc;
        self.wm.active_space_mut().map_output(&output, location);
    }

    pub(crate) fn primary_output_geometry(&self) -> Rectangle<i32, Logical> {
        self.backend
            .primary_output()
            .and_then(|output| self.space().output_geometry(output))
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (800, 600).into()))
    }

    pub(crate) fn add_toplevel(&mut self, surface: ToplevelSurface) {
        // app_id/title are not set yet at role creation; place on the active
        // canvas now and defer window-rule routing to the first buffered commit.
        let window = Window::new_wayland_window(surface);
        self.place_window(window.clone());
        self.focus_window(window.clone());
        self.pending_rules.push(window);
    }

    pub(crate) fn remove_toplevel(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_toplevel_surface(surface) else {
            return;
        };
        self.pending_rules.retain(|pending| pending != &window);
        if self.wm.remove_window(&window) {
            let focus = self.wm.active().focused().cloned();
            self.apply_focus(focus);
        }
    }

    /// Place a new window at a free, staggered spot in the viewport under the
    /// pointer. No clamping — windows may sit anywhere on the canvas.
    fn place_window(&mut self, window: Window) {
        let region = self.placement_region();
        let location = viewport::placement_location(region, self.next_window_index);
        self.next_window_index = self.next_window_index.wrapping_add(1);

        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.bounds = Some(region.size);
            });
        }

        self.space_mut().map_element(window, location, false);
        self.send_pending_window_configures();
    }

    fn placement_region(&self) -> Rectangle<i32, Logical> {
        self.space()
            .output_under(self.pointer_location)
            .next()
            .and_then(|output| self.space().output_geometry(output))
            .unwrap_or_else(|| self.primary_output_geometry())
    }

    pub(crate) fn handle_surface_commit(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window_for_toplevel_surface(surface) {
            window.on_commit();
            self.route_pending_window(&window, surface);
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

        let bounds = self.window_output_geometry(&window).size;
        toplevel.with_pending_state(|state| {
            state.bounds = Some(bounds);
        });
        toplevel.send_configure();
    }

    pub(crate) fn window_for_toplevel_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.wm.window_for(|window| {
            window
                .toplevel()
                .map(|toplevel| toplevel.wl_surface() == surface)
                .unwrap_or(false)
        })
    }

    pub(crate) fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.wm.window_for(|window| {
            let mut found = false;
            window.with_surfaces(|window_surface, _| {
                found |= window_surface == surface;
            });
            found
        })
    }

    pub(crate) fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space()
            .element_under(location)
            .and_then(|(window, window_location)| {
                window
                    .surface_under(location - window_location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_location)| {
                        (surface, (window_location + surface_location).to_f64())
                    })
            })
    }

    /// The geometry of the output a window currently overlaps, used for xdg
    /// bounds and maximize/fullscreen sizing — the window's own output, not a
    /// fixed primary.
    pub(crate) fn window_output_geometry(&self, window: &Window) -> Rectangle<i32, Logical> {
        self.output_for_window(window)
            .and_then(|output| self.space().output_geometry(&output))
            .unwrap_or_else(|| self.primary_output_geometry())
    }

    fn output_for_window(&self, window: &Window) -> Option<Output> {
        let rect = self.space().element_geometry(window)?;
        self.output_for_rect(rect)
    }

    fn output_for_rect(&self, rect: Rectangle<i32, Logical>) -> Option<Output> {
        let center = rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2));
        self.space()
            .output_under(center.to_f64())
            .next()
            .cloned()
            .or_else(|| self.focused_output())
    }

    pub(crate) fn refresh_space(&mut self) {
        self.space_mut().refresh();
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
        let result = {
            let BackendState::Udev(backend) = &mut self.backend else {
                debug!("session event ignored for non-udev backend");
                return Ok(());
            };
            backend.handle_session_event(
                event,
                &self.display_handle,
                &mut self.wm,
                &mut self.pending_output_global_removals,
            )
        };
        self.apply_output_policies();
        result
    }

    pub(crate) fn handle_udev_event(
        &mut self,
        event: UdevEvent,
        display_handle: &DisplayHandle,
        loop_handle: &LoopHandle<Self>,
    ) -> Result<(), Box<dyn Error>> {
        let result = {
            let BackendState::Udev(backend) = &mut self.backend else {
                debug!("udev event ignored for non-udev backend");
                return Ok(());
            };
            backend.handle_udev_event(
                event,
                display_handle,
                loop_handle,
                &mut self.wm,
                &mut self.pending_output_global_removals,
            )
        };
        self.apply_output_policies();
        result
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
                &mut self.wm,
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
        let Some(initial_window_location) = self.space().element_location(&window) else {
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
        let Some(initial_window_location) = self.space().element_location(&window) else {
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
        let cursor = match &self.cursor_image {
            CursorImageStatus::Surface(surface) => Some(CursorRender {
                surface,
                hotspot: cursor_hotspot(surface),
                location: self.pointer_location,
            }),
            _ => None,
        };

        match &mut self.backend {
            BackendState::Nested(backend) => {
                let size = backend.window_size();
                if size.w == 0 || size.h == 0 {
                    return Ok(());
                }

                let output = backend.output().clone();
                let damage = Rectangle::from_size(size);
                {
                    let (renderer, mut framebuffer) = backend.graphics_mut().bind()?;
                    let elements =
                        render::output_elements(renderer, self.wm.active(), &output, cursor);
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
                backend.render(self.wm.active(), cursor, BACKGROUND)?;
            }
        }

        Ok(())
    }

    pub(crate) fn frame_time(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    fn pointer_zoom(&self) -> f64 {
        self.space()
            .output_under(self.pointer_location)
            .next()
            .map(|output| self.wm.active().viewport(&output.name()).zoom)
            .unwrap_or(1.0)
    }

    fn clamp_pointer(&self, location: Point<f64, Logical>) -> Point<f64, Logical> {
        match self.wm.viewport_union() {
            Some(bounds) => clamp_pointer_location(location, bounds),
            None => location,
        }
    }

    fn focused_output(&self) -> Option<Output> {
        self.space()
            .output_under(self.pointer_location)
            .next()
            .cloned()
            .or_else(|| self.backend.primary_output().cloned())
    }

    fn collect_outputs(&self) -> Vec<Output> {
        let mut outputs = Vec::new();
        self.backend
            .for_each_output(|output| outputs.push(output.clone()));
        outputs
    }

    fn canvas_bounds(&self) -> Option<Rectangle<i32, Logical>> {
        let space = self.space();
        viewport::bounding_rect(
            space
                .elements()
                .filter_map(|element| space.element_geometry(element)),
        )
    }

    fn send_pending_window_configures(&self) {
        for window in self.space().elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }

    fn send_frame_callbacks_for_output(&self, output: &Output, time: Duration) {
        for window in self.space().elements() {
            let outputs = self.space().outputs_for_element(window);
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
        for window in self.space().elements() {
            let outputs = self.space().outputs_for_element(window);
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
                    .space()
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

fn set_window_size(window: &Window, size: Size<i32, Logical>) {
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            state.size = Some(size);
        });
        toplevel.send_pending_configure();
    }
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
