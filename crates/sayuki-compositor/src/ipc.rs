use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use calloop::PostAction;
use sayuki_ipc::{Event, EventKind, Reply, Request, encode_frame, try_decode_frame};
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

/// Per-subscriber outbound buffer cap. A subscriber whose client stops reading
/// backs up past this and is dropped, so one slow client never stalls the loop.
const SUBSCRIBER_OUTBOX_CAP: usize = 1024 * 1024;

/// Identifies one accepted IPC connection for the lifetime of its calloop
/// source, so a connection that became a subscriber can be reaped when its read
/// side closes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ConnectionId(pub(crate) u64);

struct Subscriber {
    id: ConnectionId,
    stream: UnixStream,
    kinds: Vec<EventKind>,
    outbox: Vec<u8>,
}

impl Subscriber {
    fn wants(&self, kind: EventKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    /// Drain as much of `outbox` as the socket accepts without blocking.
    /// Returns `false` when the connection is dead: a write error, or the
    /// outbox grew past the cap because the client stopped reading.
    fn flush(&mut self) -> bool {
        let mut written = 0;
        while written < self.outbox.len() {
            match self.stream.write(&self.outbox[written..]) {
                Ok(0) => return false,
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
        if written > 0 {
            self.outbox.drain(0..written);
        }
        self.outbox.len() <= SUBSCRIBER_OUTBOX_CAP
    }
}

/// Registry of event-stream subscribers. An event is encoded once and the same
/// bytes are appended to every interested subscriber's outbox.
#[derive(Default)]
pub(crate) struct Subscribers {
    entries: Vec<Subscriber>,
}

impl Subscribers {
    /// Register (or re-register) `id` as a subscriber writing through `stream`,
    /// filtered to `kinds` (empty = every kind).
    pub(crate) fn subscribe(
        &mut self,
        id: ConnectionId,
        stream: UnixStream,
        kinds: Vec<EventKind>,
    ) {
        self.remove(id);
        self.entries.push(Subscriber {
            id,
            stream,
            kinds,
            outbox: Vec::new(),
        });
    }

    /// Drop the subscriber for `id`, if any. Idempotent.
    pub(crate) fn remove(&mut self, id: ConnectionId) {
        self.entries.retain(|sub| sub.id != id);
    }

    /// Whether `id` is currently a subscriber.
    pub(crate) fn contains(&self, id: ConnectionId) -> bool {
        self.entries.iter().any(|sub| sub.id == id)
    }

    /// Whether any subscriber would receive `kind`, so the caller can skip
    /// building an event payload that nobody wants.
    pub(crate) fn any_wants(&self, kind: EventKind) -> bool {
        self.entries.iter().any(|sub| sub.wants(kind))
    }

    /// Push `event` to every interested subscriber. Dead subscribers are
    /// removed and their sockets shut down, so the read-side calloop source
    /// sees EOF and is reaped.
    pub(crate) fn broadcast(&mut self, event: &Event) {
        let kind = event.kind();
        if !self.any_wants(kind) {
            return;
        }
        let frame = match encode_frame(event) {
            Ok(frame) => frame,
            Err(error) => {
                warn!(?error, "failed to encode IPC event");
                return;
            }
        };
        let mut dead = Vec::new();
        for sub in &mut self.entries {
            if !sub.wants(kind) {
                continue;
            }
            sub.outbox.extend_from_slice(&frame);
            if !sub.flush() {
                dead.push(sub.id);
            }
        }
        for id in dead {
            self.shutdown_and_remove(id);
        }
    }

    fn shutdown_and_remove(&mut self, id: ConnectionId) {
        if let Some(pos) = self.entries.iter().position(|sub| sub.id == id) {
            let sub = self.entries.swap_remove(pos);
            let _ = sub.stream.shutdown(Shutdown::Both);
        }
    }
}

pub(crate) fn process_connection_event(
    id: ConnectionId,
    stream: &UnixStream,
    buffer: &mut Vec<u8>,
    state: &mut crate::state::SayukiState,
) -> io::Result<PostAction> {
    let mut chunk = [0u8; 8192];
    loop {
        let mut stream = stream;
        match stream.read(&mut chunk) {
            Ok(0) => {
                state.drop_ipc_subscriber(id);
                return Ok(PostAction::Remove);
            }
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => {
                warn!(?error, "failed to read IPC client");
                state.drop_ipc_subscriber(id);
                return Ok(PostAction::Remove);
            }
        }
    }

    // A subscribed connection is a one-way event stream: the client should send
    // nothing more, so discard anything it does send rather than dispatching it.
    // (EOF is still detected by the read loop above, which reaps the source.)
    if state.is_ipc_subscriber(id) {
        buffer.clear();
        return Ok(PostAction::Continue);
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
                state.drop_ipc_subscriber(id);
                return Ok(PostAction::Remove);
            }
        };

        if let Request::Subscribe { events } = request {
            match stream.try_clone() {
                Ok(writer) => state.subscribe_ipc(id, writer, events),
                Err(error) => {
                    warn!(?error, "failed to clone IPC stream for subscription");
                    return Ok(PostAction::Remove);
                }
            }
            // Now an event stream. Drop any frames pipelined after Subscribe and
            // stop dispatching requests on this connection.
            buffer.clear();
            return Ok(PostAction::Continue);
        }

        let reply = state.handle_ipc_request(request);
        if let Err(error) = write_reply(stream, &reply) {
            warn!(?error, "failed to write IPC reply");
            state.drop_ipc_subscriber(id);
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

    fn drain(reader: &mut UnixStream) {
        let mut sink = [0u8; 16384];
        loop {
            match reader.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    fn read_one_event(reader: &mut UnixStream) -> Event {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 256];
        loop {
            if let Some(event) = try_decode_frame::<Event>(&mut buffer).expect("decode") {
                return event;
            }
            let count = reader.read(&mut chunk).expect("read");
            assert!(count > 0, "stream closed before delivering event");
            buffer.extend_from_slice(&chunk[..count]);
        }
    }

    #[test]
    fn broadcast_delivers_only_matching_kinds() {
        let mut subs = Subscribers::default();
        let (window_writer, mut window_reader) = UnixStream::pair().expect("pair");
        let (action_writer, mut action_reader) = UnixStream::pair().expect("pair");
        window_writer.set_nonblocking(true).expect("nonblocking");
        action_writer.set_nonblocking(true).expect("nonblocking");
        subs.subscribe(ConnectionId(1), window_writer, vec![EventKind::Window]);
        subs.subscribe(ConnectionId(2), action_writer, vec![EventKind::Action]);

        subs.broadcast(&Event::WindowClosed {
            id: sayuki_ipc::WindowId(7),
        });

        assert_eq!(
            read_one_event(&mut window_reader),
            Event::WindowClosed {
                id: sayuki_ipc::WindowId(7)
            }
        );

        action_reader.set_nonblocking(true).expect("nonblocking");
        let mut buf = [0u8; 64];
        assert!(matches!(
            action_reader.read(&mut buf),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn empty_kind_filter_subscribes_to_every_event() {
        let mut subs = Subscribers::default();
        let (writer, mut reader) = UnixStream::pair().expect("pair");
        writer.set_nonblocking(true).expect("nonblocking");
        subs.subscribe(ConnectionId(1), writer, Vec::new());

        subs.broadcast(&Event::ConfigReloaded);
        assert_eq!(read_one_event(&mut reader), Event::ConfigReloaded);
    }

    #[test]
    fn slow_subscriber_is_dropped_without_affecting_others() {
        let mut subs = Subscribers::default();
        // Stuck client: its read end is never drained.
        let (stuck_writer, _stuck_reader) = UnixStream::pair().expect("pair");
        stuck_writer.set_nonblocking(true).expect("nonblocking");
        subs.subscribe(ConnectionId(1), stuck_writer, vec![EventKind::Config]);
        // Healthy client: drained every iteration.
        let (healthy_writer, mut healthy_reader) = UnixStream::pair().expect("pair");
        healthy_writer.set_nonblocking(true).expect("nonblocking");
        healthy_reader.set_nonblocking(true).expect("nonblocking");
        subs.subscribe(ConnectionId(2), healthy_writer, vec![EventKind::Config]);

        let message = "x".repeat(64 * 1024);
        for _ in 0..2000 {
            drain(&mut healthy_reader);
            subs.broadcast(&Event::ConfigError {
                message: message.clone(),
            });
            if subs.entries.iter().all(|sub| sub.id != ConnectionId(1)) {
                break;
            }
        }

        assert!(
            subs.entries.iter().all(|sub| sub.id != ConnectionId(1)),
            "stuck subscriber should be dropped past the outbox cap"
        );
        assert!(
            subs.entries.iter().any(|sub| sub.id == ConnectionId(2)),
            "healthy subscriber must survive a sibling being dropped"
        );
    }
}
