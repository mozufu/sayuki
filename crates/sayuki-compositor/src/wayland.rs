use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    desktop::PopupKind,
    input::{Seat, SeatHandler, pointer::Focus},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::Logical,
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};
use tracing::{debug, warn};

use crate::{
    grabs::{PointerMoveSurfaceGrab, PointerResizeSurfaceGrab},
    state::SayukiState,
};

impl BufferHandler for SayukiState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl CompositorHandler for SayukiState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("Wayland client data is always ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.handle_surface_commit(surface);
        self.popups.commit(surface);
        self.ensure_initial_configure(surface);
    }
}

impl XdgShellHandler for SayukiState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface().clone();
        self.add_toplevel(surface);
        self.ensure_initial_configure(&wl_surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });

        if let Err(error) = self.popups.track_popup(PopupKind::from(surface.clone())) {
            warn!(?error, "failed to track xdg popup");
        }
        if let Err(error) = surface.send_configure() {
            debug!(?error, "failed to configure xdg popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: wl_seat::WlSeat,
        _serial: smithay::utils::Serial,
    ) {
    }

    fn move_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: smithay::utils::Serial,
    ) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer
            .grab_start_data()
            .expect("active grab has start data");
        if !grab_started_on_surface(&start_data.focus, surface.wl_surface()) {
            return;
        }

        let Some(window) = self.window_for_toplevel_surface(surface.wl_surface()) else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };

        pointer.set_grab(
            self,
            PointerMoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            },
            serial,
            Focus::Clear,
        );
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: smithay::utils::Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        if !pointer.has_grab(serial) {
            return;
        }

        let start_data = pointer
            .grab_start_data()
            .expect("active grab has start data");
        if !grab_started_on_surface(&start_data.focus, surface.wl_surface()) {
            return;
        }

        let Some(window) = self.window_for_toplevel_surface(surface.wl_surface()) else {
            return;
        };
        let Some(initial_window_location) = self.space.element_location(&window) else {
            return;
        };

        let mut initial_window_size = window.geometry().size;
        if initial_window_size.w <= 0 || initial_window_size.h <= 0 {
            initial_window_size = (100, 100).into();
        }

        pointer.set_grab(
            self,
            PointerResizeSurfaceGrab {
                start_data,
                window,
                edges: edges.into(),
                initial_window_location,
                initial_window_size,
                last_window_size: initial_window_size,
            },
            serial,
            Focus::Clear,
        );
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel_surface(surface.wl_surface())
            && let Some(output_geometry) = self.space.output_geometry(&self.output)
        {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = Some(output_geometry.size);
            });
            self.space.map_element(window, output_geometry.loc, true);
        }
        surface.send_pending_configure();
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Maximized);
            state.size = None;
        });
        surface.send_pending_configure();
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if let Some(output_geometry) = self.space.output_geometry(&self.output) {
            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Fullscreen);
                state.size = Some(output_geometry.size);
            });
        }
        surface.send_pending_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
        });
        surface.send_pending_configure();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_toplevel(surface.wl_surface());
    }
}

impl OutputHandler for SayukiState {}

impl SelectionHandler for SayukiState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for SayukiState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for SayukiState {}
impl ServerDndGrabHandler for SayukiState {}

impl SeatHandler for SayukiState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut smithay::input::SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl ShmHandler for SayukiState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

#[derive(Default)]
pub(crate) struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        debug!(?client_id, "Wayland client initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        debug!(?client_id, ?reason, "Wayland client disconnected");
    }
}

fn grab_started_on_surface(
    focus: &Option<(WlSurface, smithay::utils::Point<f64, Logical>)>,
    surface: &WlSurface,
) -> bool {
    focus
        .as_ref()
        .map(|(focused, _)| focused.id().same_client_as(&surface.id()))
        .unwrap_or(false)
}
