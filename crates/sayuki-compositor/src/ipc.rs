use std::{
    io::{self, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use calloop::PostAction;
use sayuki_ipc::{Reply, Request, encode_frame, try_decode_frame};
use tracing::warn;

pub(crate) const SOCKET_ENV: &str = "SAYUKI_SOCKET";

pub(crate) fn socket_path(runtime_dir: &Path, wayland_display: &str) -> PathBuf {
    runtime_dir.join(format!("sayuki-{}.sock", wayland_display.replace('/', "_")))
}

pub(crate) fn runtime_socket_path(wayland_display: &str) -> io::Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR is not set; cannot create Sayuki IPC socket",
        )
    })?;

    Ok(socket_path(Path::new(&runtime_dir), wayland_display))
}

pub(crate) fn bind_listener(wayland_display: &str) -> io::Result<(PathBuf, UnixListener)> {
    let path = runtime_socket_path(wayland_display)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let listener = UnixListener::bind(&path)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o600);
    std::fs::set_permissions(&path, permissions)?;
    listener.set_nonblocking(true)?;

    Ok((path, listener))
}

pub(crate) fn process_connection_event(
    stream: &UnixStream,
    buffer: &mut Vec<u8>,
    state: &mut crate::state::SayukiState,
) -> io::Result<PostAction> {
    let mut chunk = [0u8; 8192];
    loop {
        let mut stream = stream;
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(PostAction::Remove),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => {
                warn!(?error, "failed to read IPC client");
                return Ok(PostAction::Remove);
            }
        }
    }

    loop {
        let request = match try_decode_frame::<Request>(buffer) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(PostAction::Continue),
            Err(error) => {
                let reply = Reply::Error {
                    message: error.to_string(),
                };
                let _ = write_reply(stream, &reply);
                return Ok(PostAction::Remove);
            }
        };

        let reply = state.handle_ipc_request(request);
        if let Err(error) = write_reply(stream, &reply) {
            warn!(?error, "failed to write IPC reply");
            return Ok(PostAction::Remove);
        }
    }
}

fn write_reply(stream: &UnixStream, reply: &Reply) -> io::Result<()> {
    let frame = encode_frame(reply).map_err(io::Error::other)?;
    let mut stream = stream;
    stream.write_all(&frame)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    #[test]
    fn ipc_socket_path_uses_runtime_dir_and_display() {
        assert_eq!(
            socket_path(Path::new("/run/user/1000"), "wayland-1"),
            PathBuf::from("/run/user/1000/sayuki-wayland-1.sock")
        );
    }

    #[test]
    fn ipc_socket_path_sanitizes_display_slashes() {
        assert_eq!(
            socket_path(Path::new("/tmp/runtime"), "nested/wayland-1"),
            PathBuf::from("/tmp/runtime/sayuki-nested_wayland-1.sock")
        );
    }
}
