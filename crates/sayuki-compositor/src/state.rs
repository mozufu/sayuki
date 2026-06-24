use std::{
    error::Error,
    io::{self, ErrorKind},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use calloop::{Interest, LoopHandle, Mode, generic::Generic};
use sayuki_ipc::{
    Action, Event, EventKind, OutputInfo, OutputMode, PROTOCOL_VERSION, Point as IpcPoint,
    Rect as IpcRect, Reply, Request, WindowId, WindowInfo, WorkspaceId, WorkspaceInfo,
};
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
    desktop::{
        LayerMap, LayerSurface as DesktopLayerSurface, PopupManager, Space, Window,
        WindowSurfaceType, layer_map_for_output, utils::send_frames_surface_tree,
    },
    input::{
        Seat, SeatState,
        keyboard::KeyboardHandle,
        pointer::{
            AxisFrame, ButtonEvent, CursorImageStatus, CursorImageSurfaceData, Focus,
            GrabStartData as PointerGrabStartData, MotionEvent, PointerHandle, RelativeMotionEvent,
        },
    },
    output::Output,
    reexports::wayland_server::{
        Display, DisplayHandle,
        backend::GlobalId,
        protocol::{wl_output::WlOutput, wl_surface::WlSurface},
    },
    utils::{Logical, Physical, Point, Rectangle, SERIAL_COUNTER, Size, Transform},
    wayland::{
        compositor::{CompositorState, with_states},
        cursor_shape::CursorShapeManagerState,
        foreign_toplevel_list::ForeignToplevelListState,
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        input_method::InputMethodManagerState,
        output::OutputManagerState,
        pointer_constraints::{PointerConstraintsState, with_pointer_constraint},
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        security_context::SecurityContextState,
        selection::{
            data_device::DataDeviceState,
            ext_data_control::DataControlState as ExtDataControlState,
            primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState as WlrDataControlState,
        },
        session_lock::{LockSurface, SessionLockManagerState},
        shell::{
            wlr_layer::{
                KeyboardInteractivity, Layer as WlrLayer, LayerSurface as WlrLayerSurface,
                WlrLayerShellState,
            },
            xdg::{ToplevelSurface, XdgShellState, decoration::XdgDecorationState},
        },
        shm::ShmState,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::XdgActivationState,
    },
};
use tracing::{debug, info, warn};

use crate::{
    backend::BackendState,
    config::SayukiConfig,
    grabs::{PointerMoveSurfaceGrab, PointerResizeSurfaceGrab, ResizeEdge},
    input::{actions::action_label, keybindings::KeybindingRegistry, spawn::ActionRunner},
    ipc::{ConnectionId, Subscribers},
    output::{self, OutputPolicy},
    render::{
        self, CursorRender,
        help::{HelpEntry, HelpMenu},
    },
    screencopy::{self, Screencopy, ScreencopyManagerState},
    wm::{WindowManager, focus::CycleDirection, viewport},
};

mod actions;
mod project;

const BACKGROUND: Color32F = Color32F::new(0.07, 0.08, 0.11, 1.0);

pub(crate) struct SayukiState {
    pub(crate) compositor_state: CompositorState,
    pub(crate) _output_manager_state: OutputManagerState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) layer_shell_state: WlrLayerShellState,
    pub(crate) shm_state: ShmState,
    pub(crate) data_device_state: DataDeviceState,
    pub(crate) wlr_data_control_state: WlrDataControlState,
    pub(crate) ext_data_control_state: ExtDataControlState,
    pub(crate) foreign_toplevel_list: ForeignToplevelListState,
    pub(crate) idle_notifier_state: IdleNotifierState<Self>,
    pub(crate) _idle_inhibit_state: IdleInhibitManagerState,
    pub(crate) session_lock_state: SessionLockManagerState,
    pub(crate) _xdg_decoration_state: XdgDecorationState,
    pub(crate) _fractional_scale_state: FractionalScaleManagerState,
    pub(crate) _viewporter_state: ViewporterState,
    pub(crate) _presentation_state: PresentationState,
    pub(crate) primary_selection_state: PrimarySelectionState,
    pub(crate) xdg_activation_state: XdgActivationState,
    pub(crate) _pointer_constraints_state: PointerConstraintsState,
    pub(crate) _input_method_state: InputMethodManagerState,
    pub(crate) _text_input_state: TextInputManagerState,
    pub(crate) _virtual_keyboard_state: VirtualKeyboardManagerState,
    pub(crate) _relative_pointer_state: RelativePointerManagerState,
    pub(crate) _cursor_shape_state: CursorShapeManagerState,
    pub(crate) _security_context_state: SecurityContextState,
    pub(crate) seat_state: SeatState<Self>,
    pub(crate) wm: WindowManager,
    pub(crate) popups: PopupManager,
    action_runner: ActionRunner,
    keybindings: KeybindingRegistry,
    help_menu: HelpMenu,
    help_visible: bool,

    pub(crate) display_handle: DisplayHandle,
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    backend: BackendState,
    pending_output_global_removals: Vec<(GlobalId, Instant)>,
    seat: Seat<Self>,
    pub(crate) keyboard: KeyboardHandle<Self>,
    pointer: PointerHandle<Self>,

    pub(crate) pointer_location: Point<f64, Logical>,
    cursor_image: CursorImageStatus,
    next_window_index: i32,
    next_window_id: u64,
    output_policies: Vec<OutputPolicy>,
    /// Windows awaiting one-shot window-rule routing once their client has set
    /// app_id/title (which arrive after the toplevel role is created).
    pending_rules: Vec<Window>,
    pub(crate) lock_surfaces: Vec<(LockSurface, Output)>,
    pub(crate) locked: bool,
    start_time: Instant,
    pub(crate) running: bool,
    /// Path of the loaded config file; None when using built-in defaults.
    /// Set after construction so the inotify watcher can hand it back.
    pub(crate) config_path: Option<PathBuf>,
    /// Event-stream subscribers (connections that sent `Request::Subscribe`).
    ipc_subscribers: Subscribers,
    /// Monotonic id handed to each accepted IPC connection, so a subscriber can
    /// be reaped when its read source closes.
    ipc_next_conn_id: u64,
    /// Last window id broadcast in a `WindowFocused` event, so focus changes are
    /// emitted once rather than on every `apply_focus` call.
    focused_ipc: Option<WindowId>,
    /// Owns the `zwlr_screencopy_manager_v1` global.
    _screencopy_state: ScreencopyManagerState,
    /// Captures recorded by `copy`/`copy_with_damage`, fulfilled at the end of
    /// the next render pass.
    pub(crate) pending_screencopy: Vec<Screencopy>,
}

impl SayukiState {
    pub(crate) fn new(
        display: &Display<Self>,
        config: SayukiConfig,
        backend: BackendState,
        loop_handle: LoopHandle<'static, Self>,
    ) -> Result<Self, Box<dyn Error>> {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let wlr_data_control_state =
            WlrDataControlState::new::<Self, _>(&display_handle, None, |_| true);
        let ext_data_control_state =
            ExtDataControlState::new::<Self, _>(&display_handle, None, |_| true);
        let foreign_toplevel_list = ForeignToplevelListState::new::<Self>(&display_handle);
        let idle_notifier_state =
            IdleNotifierState::<Self>::new(&display_handle, loop_handle.clone());
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&display_handle);
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&display_handle, |_| true);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&display_handle);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let presentation_state = PresentationState::new::<Self>(&display_handle, 1u32);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        let xdg_activation_state = XdgActivationState::new::<Self>(&display_handle);
        let pointer_constraints_state = PointerConstraintsState::new::<Self>(&display_handle);
        let input_method_state = InputMethodManagerState::new::<Self, _>(&display_handle, |_| true);
        let text_input_state = TextInputManagerState::new::<Self>(&display_handle);
        let virtual_keyboard_state =
            VirtualKeyboardManagerState::new::<Self, _>(&display_handle, |_| true);
        let relative_pointer_state = RelativePointerManagerState::new::<Self>(&display_handle);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&display_handle);
        let security_context_state =
            SecurityContextState::new::<Self, _>(&display_handle, |_| true);
        let screencopy_state = ScreencopyManagerState::new(&display_handle);

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
        let help_menu = HelpMenu::new(
            keybindings
                .entries()
                .map(|(keys, action)| HelpEntry {
                    keys: keys.to_owned(),
                    action: action_label(action).to_owned(),
                })
                .collect(),
        );

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
            _output_manager_state: output_manager_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            data_device_state,
            wlr_data_control_state,
            ext_data_control_state,
            foreign_toplevel_list,
            idle_notifier_state,
            _idle_inhibit_state: idle_inhibit_state,
            session_lock_state,
            _xdg_decoration_state: xdg_decoration_state,
            _fractional_scale_state: fractional_scale_state,
            _viewporter_state: viewporter_state,
            _presentation_state: presentation_state,
            primary_selection_state,
            xdg_activation_state,
            _pointer_constraints_state: pointer_constraints_state,
            _input_method_state: input_method_state,
            _text_input_state: text_input_state,
            _virtual_keyboard_state: virtual_keyboard_state,
            _relative_pointer_state: relative_pointer_state,
            _cursor_shape_state: cursor_shape_state,
            _security_context_state: security_context_state,
            seat_state,
            wm,
            popups: PopupManager::default(),
            action_runner: ActionRunner::new(),
            keybindings,
            help_menu,
            help_visible: false,
            display_handle,
            loop_handle,
            backend,
            pending_output_global_removals: Vec::new(),
            seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            cursor_image: CursorImageStatus::default_named(),
            next_window_index: 0,
            next_window_id: 0,
            output_policies,
            pending_rules: Vec::new(),
            lock_surfaces: Vec::new(),
            locked: false,
            start_time: Instant::now(),
            running: true,
            config_path: None,
            ipc_subscribers: Subscribers::default(),
            ipc_next_conn_id: 0,
            focused_ipc: None,
            _screencopy_state: screencopy_state,
            pending_screencopy: Vec::new(),
        })
    }

    /// Re-evaluate the watched config file and atomically swap all live config.
    ///
    /// If parsing or evaluation fails the error is logged and the compositor
    /// continues running with the previous config unchanged.
    pub(crate) fn reload_config(&mut self) {
        let Some(ref path) = self.config_path.clone() else {
            return;
        };

        let cfg = match SayukiConfig::load_from(path) {
            Ok(c) => c,
            Err(err) => {
                warn!(%err, path = %path.display(), "config reload failed; keeping previous config");
                self.emit_event(Event::ConfigError {
                    message: err.to_string(),
                });
                return;
            }
        };

        info!(path = %path.display(), "reloading config");

        // Keyboard: XKB layout + repeat settings.  Clone the Arc-backed handle
        // so we can pass &mut self as the data parameter without aliasing.
        let keyboard = self.keyboard.clone();
        if let Err(err) = keyboard.set_xkb_config(self, cfg.keyboard.xkb_config()) {
            warn!(?err, "failed to apply new XKB config; layout unchanged");
        }
        self.keyboard
            .change_repeat_info(cfg.keyboard.repeat_rate, cfg.keyboard.repeat_delay);

        // Keybindings + help menu.
        match KeybindingRegistry::from_configs(&cfg.keybindings) {
            Ok(registry) => {
                let entries = registry
                    .entries()
                    .map(|(keys, action)| crate::render::help::HelpEntry {
                        keys: keys.to_owned(),
                        action: crate::input::actions::action_label(action).to_owned(),
                    })
                    .collect();
                self.keybindings = registry;
                self.help_menu = crate::render::help::HelpMenu::new(entries);
            }
            Err(err) => warn!(%err, "invalid keybinding in new config; keybindings unchanged"),
        }

        // WM policy.
        self.wm.set_pan_couple(cfg.pan_couple);
        self.wm.set_snap(cfg.snap);

        // Output policies: re-apply to every current output.
        self.output_policies = cfg.outputs;
        let mut outputs = Vec::new();
        self.backend.for_each_output(|o| outputs.push(o.clone()));
        for output in &outputs {
            output::apply_policy(output, &self.output_policies);
        }

        // Notify subscribers: per-output scale/transform may have changed, and
        // the reload as a whole succeeded.
        if self.ipc_subscribers.any_wants(EventKind::Output) {
            for output in &outputs {
                let info = self.output_info(output);
                self.emit_event(Event::OutputChanged { output: info });
            }
        }
        self.emit_event(Event::ConfigReloaded);
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

    pub(crate) fn set_ipc_socket(&mut self, ipc_socket: String) {
        self.action_runner.set_ipc_socket(ipc_socket);
    }

    pub(crate) fn register_ipc_connection(
        &mut self,
        stream: UnixStream,
    ) -> Result<(), Box<dyn Error>> {
        let id = self.alloc_ipc_connection_id();
        let mut buffer = Vec::new();
        self.loop_handle.insert_source(
            Generic::new(stream, Interest::READ, Mode::Level),
            move |_, stream, state| {
                crate::ipc::process_connection_event(id, stream, &mut buffer, state)
            },
        )?;
        Ok(())
    }

    fn alloc_ipc_connection_id(&mut self) -> ConnectionId {
        let id = ConnectionId(self.ipc_next_conn_id);
        self.ipc_next_conn_id = self.ipc_next_conn_id.wrapping_add(1);
        id
    }

    /// Upgrade an IPC connection into an event-stream subscriber.
    pub(crate) fn subscribe_ipc(
        &mut self,
        id: ConnectionId,
        stream: UnixStream,
        kinds: Vec<EventKind>,
    ) {
        self.ipc_subscribers.subscribe(id, stream, kinds);
    }

    /// Drop a subscriber whose connection has closed. Idempotent.
    pub(crate) fn drop_ipc_subscriber(&mut self, id: ConnectionId) {
        self.ipc_subscribers.remove(id);
    }

    /// Whether `id` has upgraded to an event-stream subscriber.
    pub(crate) fn is_ipc_subscriber(&self, id: ConnectionId) -> bool {
        self.ipc_subscribers.contains(id)
    }

    /// Broadcast `event` to every subscriber whose filter accepts its kind.
    fn emit_event(&mut self, event: Event) {
        self.ipc_subscribers.broadcast(&event);
    }

    pub(crate) fn set_cursor_image(&mut self, image: CursorImageStatus) {
        self.cursor_image = image;
    }

    pub(crate) fn is_locked(&self) -> bool {
        self.locked
    }

    pub(crate) fn lock_surface_under(&self, _location: Point<f64, Logical>) -> Option<WlSurface> {
        self.lock_surfaces
            .iter()
            .find(|(surface, _)| surface.alive())
            .map(|(surface, _)| surface.wl_surface().clone())
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
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
        let action = keyboard.input::<Action, _>(
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
            self.dispatch_action(action);
        }
    }

    pub(crate) fn dispatch_action(&mut self, action: Action) {
        if self.ipc_subscribers.any_wants(EventKind::Action) {
            self.emit_event(Event::ActionInvoked {
                action: action.clone(),
            });
        }
        match action {
            Action::Noop => {}
            Action::Quit => {
                info!("quit action requested");
                self.running = false;
            }
            Action::Spawn { argv } => self.spawn_in_active(&argv),
            Action::BeginMove => self.begin_move_focused_window(),
            Action::BeginResize { edges } => {
                self.begin_resize_focused_window(Self::resize_edge_from_ipc(edges))
            }
            Action::SwitchWorkspace { workspace } => {
                self.switch_workspace(Self::workspace_ref_from_ipc(workspace))
            }
            Action::MoveToWorkspace { workspace } => {
                self.move_focused_to_workspace(Self::workspace_ref_from_ipc(workspace))
            }
            Action::PanViewport { dx, dy } => self.pan_viewport(Point::from((dx, dy))),
            Action::ZoomViewport { factor } => self.zoom_viewport(factor),
            Action::ToggleOverview => self.toggle_overview(),
            Action::ToggleMinimap => self.toggle_minimap(),
            Action::TogglePin => self.toggle_pin_focused(),
            Action::SwapWindow { target } => self.swap_focused(Self::swap_target_from_ipc(target)),
            Action::FocusNext => self.cycle_focus(CycleDirection::Forward),
            Action::FocusPrev => self.cycle_focus(CycleDirection::Backward),
            Action::ToggleHelp => self.help_visible = !self.help_visible,
        }
    }

    pub(crate) fn handle_ipc_request(&mut self, request: Request) -> Reply {
        match request {
            Request::GetVersion => Reply::Version {
                compositor: env!("CARGO_PKG_VERSION").to_owned(),
                protocol: PROTOCOL_VERSION,
            },
            Request::GetWindows => Reply::Windows {
                windows: self.window_snapshot(),
            },
            Request::GetWorkspaces => Reply::Workspaces {
                workspaces: self.workspace_snapshot(),
            },
            Request::GetOutputs => Reply::Outputs {
                outputs: self.output_snapshot(),
            },
            Request::GetFocused => self.focused_reply(),
            Request::Action { action } => {
                self.dispatch_action(action);
                Reply::Ok
            }
            // Subscribe is intercepted at the connection layer
            // (`ipc::process_connection_event`) and never reaches here.
            Request::Subscribe { .. } => Reply::Error {
                message: "subscribe is handled at the connection layer".to_owned(),
            },
        }
    }

    fn window_snapshot(&self) -> Vec<WindowInfo> {
        self.wm
            .canvases()
            .flat_map(|canvas| {
                canvas
                    .space()
                    .elements()
                    .filter_map(move |window| Self::window_info_on(canvas, window))
            })
            .collect()
    }

    /// Build the IPC `WindowInfo` for `window` as it sits on `canvas`.
    fn window_info_on(canvas: &crate::wm::Canvas, window: &Window) -> Option<WindowInfo> {
        let id = window_id(window)?;
        let (app_id, title) = project::window_identity(window);
        Some(WindowInfo {
            id,
            app_id,
            title,
            workspace: WorkspaceId(canvas.id().raw()),
            floating: true,
            focused: canvas.focused() == Some(window),
            geometry: canvas.space().element_geometry(window).map(rect_from),
        })
    }

    /// Find `window` across canvases and build its `WindowInfo`.
    fn window_info_for(&self, window: &Window) -> Option<WindowInfo> {
        self.wm
            .canvases()
            .find(|canvas| canvas.space().elements().any(|element| element == window))
            .and_then(|canvas| Self::window_info_on(canvas, window))
    }

    fn workspace_snapshot(&self) -> Vec<WorkspaceInfo> {
        self.wm
            .canvases()
            .map(|canvas| WorkspaceInfo {
                id: WorkspaceId(canvas.id().raw()),
                name: canvas.name().to_owned(),
                project_path: canvas.working_dir().map(|path| path.display().to_string()),
                active: self.wm.is_active(canvas.id()),
                window_ids: canvas.space().elements().filter_map(window_id).collect(),
            })
            .collect()
    }

    fn output_snapshot(&self) -> Vec<OutputInfo> {
        self.collect_outputs()
            .into_iter()
            .map(|output| self.output_info(&output))
            .collect()
    }

    /// Build the IPC `OutputInfo` describing `output`'s current mode, scale,
    /// transform, position, and work area.
    fn output_info(&self, output: &Output) -> OutputInfo {
        let properties = output.physical_properties();
        OutputInfo {
            name: output.name(),
            make: properties.make,
            model: properties.model,
            mode: output.current_mode().map(|mode| OutputMode {
                width: mode.size.w,
                height: mode.size.h,
                refresh: mode.refresh,
            }),
            scale: output.current_scale().fractional_scale(),
            transform: output::transform_label(output.current_transform()).to_owned(),
            position: {
                let location = output.current_location();
                IpcPoint {
                    x: location.x,
                    y: location.y,
                }
            },
            work_area: self.output_work_area(output).map(rect_from),
        }
    }

    fn focused_reply(&self) -> Reply {
        Reply::Focused {
            window: self.wm.active().focused().and_then(window_id),
            workspace: WorkspaceId(self.wm.active().id().raw()),
        }
    }

    fn workspace_ref_from_ipc(reference: sayuki_ipc::WorkspaceRef) -> crate::wm::WorkspaceRef {
        match reference {
            sayuki_ipc::WorkspaceRef::Index(index) => crate::wm::WorkspaceRef::Index(index),
            sayuki_ipc::WorkspaceRef::Name(name) => crate::wm::WorkspaceRef::Name(name),
        }
    }

    fn resize_edge_from_ipc(edge: sayuki_ipc::ResizeEdge) -> ResizeEdge {
        match edge {
            sayuki_ipc::ResizeEdge::None => ResizeEdge::NONE,
            sayuki_ipc::ResizeEdge::Top => ResizeEdge::TOP,
            sayuki_ipc::ResizeEdge::Bottom => ResizeEdge::BOTTOM,
            sayuki_ipc::ResizeEdge::Left => ResizeEdge::LEFT,
            sayuki_ipc::ResizeEdge::Right => ResizeEdge::RIGHT,
            sayuki_ipc::ResizeEdge::TopLeft => ResizeEdge::TOP_LEFT,
            sayuki_ipc::ResizeEdge::TopRight => ResizeEdge::TOP_RIGHT,
            sayuki_ipc::ResizeEdge::BottomLeft => ResizeEdge::BOTTOM_LEFT,
            sayuki_ipc::ResizeEdge::BottomRight => ResizeEdge::BOTTOM_RIGHT,
        }
    }

    fn swap_target_from_ipc(target: sayuki_ipc::SwapTarget) -> crate::wm::swap::SwapTarget {
        match target {
            sayuki_ipc::SwapTarget::Direction { direction } => {
                crate::wm::swap::SwapTarget::Direction(Self::direction_from_ipc(direction))
            }
            sayuki_ipc::SwapTarget::Next => crate::wm::swap::SwapTarget::Next,
            sayuki_ipc::SwapTarget::Prev => crate::wm::swap::SwapTarget::Prev,
        }
    }

    fn direction_from_ipc(direction: sayuki_ipc::Direction) -> crate::wm::swap::Direction {
        match direction {
            sayuki_ipc::Direction::Left => crate::wm::swap::Direction::Left,
            sayuki_ipc::Direction::Right => crate::wm::swap::Direction::Right,
            sayuki_ipc::Direction::Up => crate::wm::swap::Direction::Up,
            sayuki_ipc::Direction::Down => crate::wm::swap::Direction::Down,
        }
    }

    fn forward_pointer_relative_motion<B>(&mut self, event: B::PointerMotionEvent)
    where
        B: InputBackend,
    {
        // A physical mouse delta should move the cursor the same screen distance
        // regardless of zoom, so divide the delta by the zoom under the pointer.
        let zoom = self.pointer_zoom();
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
        let delta = event.delta();
        let under = self.surface_under(self.pointer_location);
        let pointer = self.pointer.clone();
        pointer.relative_motion(
            self,
            under.clone(),
            &RelativeMotionEvent {
                delta,
                delta_unaccel: event.delta_unaccel(),
                utime: event.time(),
            },
        );
        if self.pointer_focus_constrained(&under) {
            pointer.frame(self);
            return;
        }
        let scaled = Point::from((delta.x / zoom, delta.y / zoom));
        let location = self.clamp_pointer(self.pointer_location + scaled);
        self.pointer_location = location;
        self.forward_pointer_motion_to_clients(location, event.time_msec());
    }

    fn forward_pointer_absolute_motion<B>(&mut self, event: B::PointerMotionAbsoluteEvent)
    where
        B: InputBackend,
    {
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
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

    fn pointer_focus_constrained(&self, under: &Option<(WlSurface, Point<f64, Logical>)>) -> bool {
        let Some((surface, _)) = under else {
            return false;
        };
        with_pointer_constraint(surface, &self.pointer, |constraint| {
            constraint
                .as_deref()
                .map(|constraint| constraint.is_active())
                .unwrap_or(false)
        })
    }

    fn forward_pointer_button<B>(&mut self, event: B::PointerButtonEvent)
    where
        B: InputBackend,
    {
        let serial = SERIAL_COUNTER.next_serial();
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
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
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
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
        if self.ipc_subscribers.any_wants(EventKind::Output) {
            let info = self.output_info(&output);
            self.emit_event(Event::OutputChanged { output: info });
        }
    }

    pub(crate) fn add_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.focused_output());
        let Some(output) = output else {
            debug!(namespace, "layer surface ignored because no output exists");
            return;
        };

        surface.with_pending_state(|state| {
            state.size = self
                .space()
                .output_geometry(&output)
                .map(|geometry| geometry.size);
        });
        let layer = DesktopLayerSurface::new(surface.clone(), namespace);
        let mut layer_map = layer_map_for_output(&output);
        if let Err(error) = layer_map.map_layer(&layer) {
            debug!(?error, output = %output.name(), "failed to map layer surface");
        }
    }

    pub(crate) fn remove_layer_surface(&mut self, layer: &WlrLayerSurface) {
        let surface = layer.wl_surface();
        self.for_each_layer_map(|layer_map| {
            if let Some(layer) = layer_map
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .cloned()
            {
                layer_map.unmap_layer(&layer);
            }
        });
    }

    pub(crate) fn arrange_layer_for_surface(&mut self, surface: &WlSurface) {
        self.for_each_layer_map(|layer_map| {
            if let Some(layer) = layer_map
                .layer_for_surface(surface, WindowSurfaceType::ALL)
                .cloned()
            {
                layer_map.arrange();
                layer.layer_surface().send_pending_configure();
            }
        });
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
        self.assign_window_id(&window);
        self.register_foreign_toplevel(&window);
        self.place_window(window.clone());
        if let Some(info) = self.window_info_for(&window) {
            self.emit_event(Event::WindowOpened { window: info });
        }
        self.focus_window(window.clone());
        self.pending_rules.push(window);
    }

    fn assign_window_id(&mut self, window: &Window) {
        let id = WindowId(self.next_window_id);
        self.next_window_id = self.next_window_id.wrapping_add(1);
        window.user_data().insert_if_missing(|| id);
    }
    pub(crate) fn remove_toplevel(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_toplevel_surface(surface) else {
            return;
        };
        self.unregister_foreign_toplevel(&window);
        self.pending_rules.retain(|pending| pending != &window);
        if self.wm.remove_window(&window) {
            if let Some(id) = window_id(&window) {
                self.emit_event(Event::WindowClosed { id });
            }
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
            .and_then(|output| self.output_work_area(output))
            .unwrap_or_else(|| self.primary_work_area())
    }

    pub(crate) fn handle_surface_commit(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window_for_toplevel_surface(surface) {
            window.on_commit();
            self.refresh_foreign_toplevel(&window);
            self.route_pending_window(&window, surface);
        } else if let Some(window) = self.window_for_surface(surface) {
            window.on_commit();
        } else {
            self.arrange_layer_for_surface(surface);
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
        if self.is_locked() {
            return self
                .lock_surface_under(location)
                .map(|surface| (surface, location));
        }
        self.layer_surface_under(location, &[WlrLayer::Overlay, WlrLayer::Top])
            .or_else(|| {
                self.space()
                    .element_under(location)
                    .and_then(|(window, window_location)| {
                        window
                            .surface_under(
                                location - window_location.to_f64(),
                                WindowSurfaceType::ALL,
                            )
                            .map(|(surface, surface_location)| {
                                (surface, (window_location + surface_location).to_f64())
                            })
                    })
            })
            .or_else(|| {
                self.layer_surface_under(location, &[WlrLayer::Bottom, WlrLayer::Background])
            })
    }

    /// The geometry of the output a window currently overlaps, used for xdg
    /// bounds and maximize/fullscreen sizing — the window's own output, not a
    /// fixed primary.
    pub(crate) fn window_output_geometry(&self, window: &Window) -> Rectangle<i32, Logical> {
        self.output_for_window(window)
            .and_then(|output| self.output_work_area(&output))
            .unwrap_or_else(|| self.primary_work_area())
    }

    pub(crate) fn window_full_output_geometry(&self, window: &Window) -> Rectangle<i32, Logical> {
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
        self.lock_surfaces.retain(|(surface, _)| surface.alive());
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
        let help_menu = self.help_visible.then_some(&self.help_menu);
        let locked = self.is_locked();

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
                    let elements = render::output_elements(
                        renderer,
                        self.wm.active(),
                        &output,
                        cursor,
                        help_menu,
                        &self.lock_surfaces,
                        locked,
                    );
                    let mut frame =
                        renderer.render(&mut framebuffer, size, Transform::Flipped180)?;
                    frame.clear(BACKGROUND, &[damage])?;
                    draw_render_elements(&mut frame, 1.0, &elements, &[damage])?;
                    let _sync_point = frame.finish()?;
                }
                backend.graphics_mut().submit(Some(&[damage]))?;
                self.discard_presentation_feedback_for_all_outputs();
                self.send_frame_callbacks_for_all_outputs();
            }
            BackendState::Udev(backend) => {
                backend.render(
                    self.wm.active(),
                    cursor,
                    BACKGROUND,
                    help_menu,
                    &self.lock_surfaces,
                    locked,
                )?;
            }
        }
        self.fulfill_screencopy();

        Ok(())
    }

    /// Drain pending screencopy captures: render each output into an offscreen
    /// texture and copy the requested region into the client's SHM buffer. Runs
    /// after the on-screen render so captures observe the frame just drawn.
    fn fulfill_screencopy(&mut self) {
        if self.pending_screencopy.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_screencopy);
        let cursor = match &self.cursor_image {
            CursorImageStatus::Surface(surface) => Some(CursorRender {
                surface,
                hotspot: cursor_hotspot(surface),
                location: self.pointer_location,
            }),
            _ => None,
        };
        let help_menu = self.help_visible.then_some(&self.help_menu);
        let locked = self.locked;
        let canvas = self.wm.active();
        let lock_surfaces = self.lock_surfaces.as_slice();

        for capture in pending {
            let overlay_cursor = if capture.overlay_cursor { cursor } else { None };
            let Some(renderer) = self.backend.renderer_for_output(&capture.output) else {
                capture.frame.failed();
                continue;
            };
            let elements = render::output_elements(
                renderer,
                canvas,
                &capture.output,
                overlay_cursor,
                help_menu,
                lock_surfaces,
                locked,
            );
            match screencopy::render_capture(renderer, &elements, &capture, BACKGROUND) {
                Ok(()) => screencopy::send_ready(&capture),
                Err(_) => capture.frame.failed(),
            }
        }
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

    fn primary_work_area(&self) -> Rectangle<i32, Logical> {
        self.backend
            .primary_output()
            .and_then(|output| self.output_work_area(output))
            .unwrap_or_else(|| self.primary_output_geometry())
    }

    fn output_work_area(&self, output: &Output) -> Option<Rectangle<i32, Logical>> {
        let output_geometry = self.space().output_geometry(output)?;
        let mut layer_map = layer_map_for_output(output);
        layer_map.arrange();
        let zone = layer_map.non_exclusive_zone();
        Some(Rectangle::new(output_geometry.loc + zone.loc, zone.size))
    }

    fn layer_surface_under(
        &self,
        location: Point<f64, Logical>,
        layers: &[WlrLayer],
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        for output in self.space().output_under(location) {
            let output_geometry = self.space().output_geometry(output)?;
            let output_location = location - output_geometry.loc.to_f64();
            let layer_map = layer_map_for_output(output);
            for layer in layers {
                if let Some(surface) = layer_map.layer_under(*layer, output_location) {
                    let geometry = layer_map.layer_geometry(surface)?;
                    return surface
                        .surface_under(
                            output_location - geometry.loc.to_f64(),
                            WindowSurfaceType::ALL,
                        )
                        .map(|(surface, surface_location)| {
                            (
                                surface,
                                (output_geometry.loc + geometry.loc + surface_location).to_f64(),
                            )
                        });
                }
            }
        }
        None
    }

    pub(crate) fn layer_keyboard_focus_under(
        &self,
        location: Point<f64, Logical>,
        layers: &[WlrLayer],
    ) -> Option<WlSurface> {
        for output in self.space().output_under(location) {
            let output_geometry = self.space().output_geometry(output)?;
            let output_location = location - output_geometry.loc.to_f64();
            let layer_map = layer_map_for_output(output);
            for layer in layers {
                if let Some(surface) = layer_map.layer_under(*layer, output_location)
                    && surface.can_receive_keyboard_focus()
                {
                    return Some(surface.wl_surface().clone());
                }
            }
        }
        None
    }

    pub(crate) fn exclusive_layer_focus(&self) -> Option<WlSurface> {
        for output in self.collect_outputs() {
            let layer_map = layer_map_for_output(&output);
            for layer in [WlrLayer::Overlay, WlrLayer::Top] {
                if let Some(surface) = layer_map.layers_on(layer).rev().find(|surface| {
                    surface.cached_state().keyboard_interactivity
                        == KeyboardInteractivity::Exclusive
                }) {
                    return Some(surface.wl_surface().clone());
                }
            }
        }
        None
    }

    fn for_each_layer_map(&self, mut f: impl FnMut(&mut LayerMap)) {
        self.backend.for_each_output(|output| {
            let mut layer_map = layer_map_for_output(output);
            f(&mut layer_map);
        });
    }

    fn send_frame_callbacks_for_output(&self, output: &Output, time: Duration) {
        for layer in layer_map_for_output(output).layers() {
            layer.send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        for (lock_surface, lock_output) in &self.lock_surfaces {
            if lock_output == output {
                send_frames_surface_tree(
                    lock_surface.wl_surface(),
                    output,
                    time,
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            }
        }

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
        for output in self.collect_outputs() {
            for layer in layer_map_for_output(&output).layers() {
                layer.send_frame(&output, time, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
            for (lock_surface, lock_output) in &self.lock_surfaces {
                if lock_output == &output {
                    send_frames_surface_tree(
                        lock_surface.wl_surface(),
                        &output,
                        time,
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                }
            }
        }

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

    fn discard_presentation_feedback_for_all_outputs(&self) {
        for output in self.collect_outputs() {
            let mut feedback = smithay::desktop::utils::OutputPresentationFeedback::new(&output);
            for window in self.space().elements() {
                window.take_presentation_feedback(
                    &mut feedback,
                    |_, _| Some(output.clone()),
                    |_, _| smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::empty(),
                );
            }
            feedback.discarded();
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

pub(crate) fn window_id(window: &Window) -> Option<WindowId> {
    window.user_data().get::<WindowId>().copied()
}

fn rect_from(rectangle: Rectangle<i32, Logical>) -> IpcRect {
    IpcRect {
        x: rectangle.loc.x,
        y: rectangle.loc.y,
        width: rectangle.size.w,
        height: rectangle.size.h,
    }
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
