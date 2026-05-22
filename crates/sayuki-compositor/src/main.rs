use std::{error::Error, sync::Arc, time::Duration};

use calloop::EventLoop;
use clap::Parser;
use smithay::{
    backend::{renderer::gles::GlesRenderer, winit},
    delegate_compositor, delegate_output, delegate_seat, delegate_shm,
    reexports::wayland_server::Display,
    wayland::socket::ListeningSocketSource,
};
use tracing::{debug, error, info};

use crate::{cli::Args, logging::init_tracing, state::SayukiState, wayland::ClientState};

mod cli;
mod logging;
mod output;
mod state;
mod wayland;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let args = Args::parse();
    let mut event_loop = EventLoop::<SayukiState>::try_new()?;
    let mut display = Display::<SayukiState>::new()?;

    let (backend, winit_event_loop) = winit::init::<GlesRenderer>()?;
    let mut state = SayukiState::new(&display, backend)?;

    let socket_source = match args.socket.as_deref() {
        Some(socket_name) => ListeningSocketSource::with_name(socket_name)?,
        None => ListeningSocketSource::new_auto()?,
    };
    let socket_name = socket_source.socket_name().to_string_lossy().into_owned();

    let loop_handle = event_loop.handle();

    loop_handle.insert_source(winit_event_loop, |event, _, state| {
        state.handle_winit_event(event);
    })?;

    let mut client_display_handle = display.handle();
    loop_handle.insert_source(socket_source, move |client_stream, _, _state| {
        match client_display_handle.insert_client(client_stream, Arc::new(ClientState::default())) {
            Ok(client) => debug!(client = ?client.id(), "accepted Wayland client"),
            Err(error) => error!(?error, "failed to accept Wayland client"),
        }
    })?;

    info!(socket = %socket_name, "Sayuki is listening for Wayland clients");
    println!("Sayuki is running. Start clients with WAYLAND_DISPLAY={socket_name}");

    while state.running {
        event_loop.dispatch(Some(FRAME_INTERVAL), &mut state)?;
        display.dispatch_clients(&mut state)?;
        state.output.cleanup();
        display.flush_clients()?;
        state.render()?;
    }

    Ok(())
}

delegate_compositor!(SayukiState);
delegate_output!(SayukiState);
delegate_seat!(SayukiState);
delegate_shm!(SayukiState);
