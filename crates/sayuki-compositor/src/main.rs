use std::{error::Error, sync::Arc, time::Duration};

use calloop::EventLoop;
use clap::Parser;
use smithay::{
    delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device,
    delegate_ext_data_control, delegate_foreign_toplevel_list, delegate_fractional_scale,
    delegate_idle_inhibit, delegate_idle_notify, delegate_input_method_manager,
    delegate_layer_shell, delegate_output, delegate_pointer_constraints, delegate_presentation,
    delegate_primary_selection, delegate_relative_pointer, delegate_seat,
    delegate_security_context, delegate_session_lock, delegate_shm, delegate_text_input_manager,
    delegate_viewporter, delegate_virtual_keyboard_manager, delegate_xdg_activation,
    delegate_xdg_decoration, delegate_xdg_shell, reexports::wayland_server::Display,
    wayland::socket::ListeningSocketSource,
};
use tracing::{debug, error, info};

use crate::{
    backend::BackendState,
    cli::{Args, BackendKind},
    config::SayukiConfig,
    logging::init_tracing,
    state::SayukiState,
    wayland::ClientState,
};

mod backend;
mod cli;
mod config;
mod grabs;
mod input;
mod logging;
mod output;
mod project;
mod render;
mod state;
mod wayland;
mod wm;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let args = Args::parse();
    let config = SayukiConfig::load(args.config.as_deref())?;
    let mut event_loop = EventLoop::<SayukiState>::try_new()?;
    let mut display = Display::<SayukiState>::new()?;

    let loop_handle = event_loop.handle();
    let display_handle = display.handle();
    let backend = match args.backend {
        BackendKind::Nested => {
            let (backend, winit_event_loop) = backend::nested::init(&display_handle)?;
            loop_handle.insert_source(winit_event_loop, |event, _, state| {
                state.handle_winit_event(event);
            })?;
            BackendState::Nested(backend)
        }
        BackendKind::Udev => BackendState::Udev(backend::udev::NativeBackend::init(
            &display_handle,
            &loop_handle,
        )?),
    };
    let mut state = SayukiState::new(&display, config, backend, loop_handle.clone())?;

    let socket_source = match args.socket.as_deref() {
        Some(socket_name) => ListeningSocketSource::with_name(socket_name)?,
        None => ListeningSocketSource::new_auto()?,
    };
    let socket_name = socket_source.socket_name().to_string_lossy().into_owned();

    let mut client_display_handle = display.handle();
    loop_handle.insert_source(socket_source, move |client_stream, _, _state| {
        match client_display_handle.insert_client(client_stream, Arc::new(ClientState::default())) {
            Ok(client) => debug!(client = ?client.id(), "accepted Wayland client"),
            Err(error) => error!(?error, "failed to accept Wayland client"),
        }
    })?;

    state.set_wayland_display(socket_name.clone());

    info!(socket = %socket_name, "Sayuki is listening for Wayland clients");
    println!("Sayuki is running. Start clients with WAYLAND_DISPLAY={socket_name}");

    while state.running {
        event_loop.dispatch(Some(FRAME_INTERVAL), &mut state)?;
        display.dispatch_clients(&mut state)?;
        state.refresh_space();
        display.flush_clients()?;
        state.render()?;
    }

    Ok(())
}

delegate_compositor!(SayukiState);
delegate_data_device!(SayukiState);
delegate_data_control!(SayukiState);
delegate_ext_data_control!(SayukiState);
delegate_foreign_toplevel_list!(SayukiState);
delegate_layer_shell!(SayukiState);
delegate_output!(SayukiState);
delegate_seat!(SayukiState);
delegate_shm!(SayukiState);
delegate_xdg_shell!(SayukiState);
delegate_cursor_shape!(SayukiState);
delegate_fractional_scale!(SayukiState);
delegate_idle_inhibit!(SayukiState);
delegate_idle_notify!(SayukiState);
delegate_input_method_manager!(SayukiState);
delegate_pointer_constraints!(SayukiState);
delegate_presentation!(SayukiState);
delegate_primary_selection!(SayukiState);
delegate_relative_pointer!(SayukiState);
delegate_security_context!(SayukiState);
delegate_session_lock!(SayukiState);
delegate_text_input_manager!(SayukiState);
delegate_viewporter!(SayukiState);
delegate_virtual_keyboard_manager!(SayukiState);
delegate_xdg_activation!(SayukiState);
delegate_xdg_decoration!(SayukiState);
