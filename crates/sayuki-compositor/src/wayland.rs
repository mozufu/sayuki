use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    input::{Seat, SeatHandler},
    reexports::wayland_server::{
        Client,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
            TraversalAction, with_surface_tree_downward,
        },
        output::OutputHandler,
        shm::{ShmHandler, ShmState},
    },
};
use tracing::debug;

use crate::state::SayukiState;

fn send_frame_callbacks(surface: &WlSurface, time: u32) {
    with_surface_tree_downward(
        surface,
        (),
        |_, _, &()| TraversalAction::DoChildren(()),
        |_surface, states, &()| {
            for callback in states
                .cached_state
                .get::<SurfaceAttributes>()
                .current()
                .frame_callbacks
                .drain(..)
            {
                callback.done(time);
            }
        },
        |_, _, &()| true,
    );
}

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
        self.output.enter(surface);
        send_frame_callbacks(surface, self.frame_time());
    }
}

impl OutputHandler for SayukiState {}

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
