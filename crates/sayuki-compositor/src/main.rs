use std::{error::Error, ffi::OsStr, sync::Arc, time::Duration};

use calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use inotify::{Inotify, WatchMask};
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
mod foreign_toplevel;
mod grabs;
mod input;
mod ipc;
mod logging;
mod output;
mod project;
mod render;
mod screencopy;
mod state;
mod wayland;
mod wm;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn main() -> Result<(), Box<dyn Error>> {
    init_tracing();

    let args = Args::parse();
    let (config, config_path) = SayukiConfig::load(args.config.as_deref())?;
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
    state.config_path = config_path;

    let socket_source = match args.socket.as_deref() {
        Some(socket_name) => ListeningSocketSource::with_name(socket_name)?,
        None => ListeningSocketSource::new_auto()?,
    };
    let socket_name = socket_source.socket_name().to_string_lossy().into_owned();
    let (ipc_socket_path, ipc_listener) = ipc::bind_listener(&socket_name)?;

    loop_handle.insert_source(
        Generic::new(ipc_listener, Interest::READ, Mode::Level),
        |_, listener, state| {
            loop {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        if let Err(error) = stream.set_nonblocking(true) {
                            error!(?error, "failed to make IPC client nonblocking");
                            continue;
                        }
                        if let Err(error) = state.register_ipc_connection(stream) {
                            error!(?error, "failed to register IPC client");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(PostAction::Continue);
                    }
                    Err(error) => error!(?error, "failed to accept IPC client"),
                }
            }
        },
    )?;

    let mut client_display_handle = display.handle();
    loop_handle.insert_source(socket_source, move |client_stream, _, _state| {
        match client_display_handle.insert_client(client_stream, Arc::new(ClientState::default())) {
            Ok(client) => debug!(client = ?client.id(), "accepted Wayland client"),
            Err(error) => error!(?error, "failed to accept Wayland client"),
        }
    })?;

    // Config hot-reload: a background thread blocks on inotify, filtering for
    // IN_CLOSE_WRITE and IN_MOVED_TO on the config file's parent directory.
    // A calloop channel delivers the trigger back to the event loop thread.
    // Thread-per-watcher is simpler than calloop::generic::Generic<Inotify>
    // because inotify::Inotify is not DerefMut through NoIoDrop.
    if let Some(ref path) = state.config_path.clone()
        && let Some(dir) = path.parent()
    {
        let dir = dir.to_owned();
        let config_filename = path.file_name().map(OsStr::to_owned);
        match Inotify::init() {
            Err(err) => error!(?err, "failed to initialise inotify for config hot-reload"),
            Ok(mut inotify) => {
                match inotify.watches().add(&dir, WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO) {
                    Err(err) => error!(?err, "failed to add inotify watch for config dir"),
                    Ok(_) => {
                        let (tx, rx) = calloop::channel::channel::<()>();
                        std::thread::Builder::new()
                            .name("config-watcher".into())
                            .spawn(move || {
                                let mut buf = vec![0u8; 4096];
                                loop {
                                    let mut events = match inotify.read_events_blocking(&mut buf) {
                                        Ok(ev) => ev,
                                        Err(_) => break,
                                    };
                                    let triggered = events.any(|ev| {
                                        ev.name.map(|n| n.to_owned()) == config_filename
                                    });
                                    if triggered && tx.send(()).is_err() {
                                        break; // main loop exited, drop watcher
                                    }
                                }
                            })
                            .expect("failed to spawn config-watcher thread");
                        match loop_handle.insert_source(rx, |_, _, state| {
                            state.reload_config();
                        }) {
                            Err(err) => error!(?err, "failed to register config reload channel"),
                            Ok(_) => info!(path = %path.display(), "watching config for hot-reload"),
                        }
                    }
                }
            }
        }
    }

    state.set_wayland_display(socket_name.clone());
    state.set_ipc_socket(ipc_socket_path.to_string_lossy().into_owned());

    info!(
        wayland_display = %socket_name,
        sayuki_socket = %ipc_socket_path.display(),
        "Sayuki is listening for Wayland and IPC clients"
    );
    println!(
        "Sayuki is running. Start clients with WAYLAND_DISPLAY={socket_name}; SAYUKI_SOCKET={}",
        ipc_socket_path.display()
    );

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
