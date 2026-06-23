use std::{
    error::Error,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use sayuki_ipc::{
    Action, FrameError, OutputInfo, Reply, Request, WindowInfo, WorkspaceInfo, encode_frame,
    try_decode_frame,
};

#[derive(Debug, Parser)]
#[command(version, about = "Control a running Sayuki compositor")]
struct Args {
    #[arg(long, env = "SAYUKI_SOCKET")]
    socket: Option<PathBuf>,
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send a protocol-version IPC request.
    Version,
    /// List windows.
    Windows,
    /// List workspaces.
    Workspaces,
    /// List outputs.
    Outputs,
    /// Show the focused window and workspace.
    Focused,
    /// Send a quit action IPC request.
    Quit,
    /// Send a spawn action IPC request.
    Spawn {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        argv: Vec<String>,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let request = request_from_command(args.command);
    let socket_path = require_socket_path(args.socket)?;
    let reply = request_reply(&socket_path, &request)?;

    print!("{}", render_reply(&reply, args.json));
    Ok(())
}

fn request_from_command(command: Command) -> Request {
    match command {
        Command::Version => Request::GetVersion,
        Command::Windows => Request::GetWindows,
        Command::Workspaces => Request::GetWorkspaces,
        Command::Outputs => Request::GetOutputs,
        Command::Focused => Request::GetFocused,
        Command::Quit => Request::Action {
            action: Action::Quit,
        },
        Command::Spawn { argv } => Request::Action {
            action: Action::Spawn { argv },
        },
    }
}

fn require_socket_path(socket: Option<PathBuf>) -> io::Result<PathBuf> {
    socket.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "SAYUKI_SOCKET is not set; pass --socket <path> or run sayukictl from a Sayuki-spawned process",
        )
    })
}

fn frame_error_to_io(error: FrameError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn request_reply(socket_path: &Path, request: &Request) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(socket_path)?;
    let frame = encode_frame(request).map_err(frame_error_to_io)?;
    stream.write_all(&frame)?;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match try_decode_frame::<Reply>(&mut buffer) {
            Ok(Some(reply)) => return Ok(reply),
            Ok(None) => {}
            Err(error) => return Err(frame_error_to_io(error)),
        }

        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "IPC server closed before replying",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

fn render_reply(reply: &Reply, json: bool) -> String {
    if json {
        return serde_json::to_string_pretty(reply).expect("serialize reply") + "\n";
    }

    match reply {
        Reply::Ok => "ok\n".to_owned(),
        Reply::Error { message } => format!("error: {message}\n"),
        Reply::Version {
            compositor,
            protocol,
        } => format!("sayuki {compositor} (protocol {protocol})\n"),
        Reply::Windows { windows } => render_windows(windows),
        Reply::Workspaces { workspaces } => render_workspaces(workspaces),
        Reply::Outputs { outputs } => render_outputs(outputs),
        Reply::Focused { window, workspace } => format!(
            "focused window {} on workspace {}\n",
            window
                .map(|window| window.0.to_string())
                .unwrap_or_else(|| "none".to_owned()),
            workspace.0
        ),
    }
}

fn render_windows(windows: &[WindowInfo]) -> String {
    if windows.is_empty() {
        return "(none)\n".to_owned();
    }

    let mut output = "ID  WS  APP-ID  TITLE  GEOMETRY  FOCUSED\n".to_owned();
    for window in windows {
        let geometry = window
            .geometry
            .as_ref()
            .map(|rect| format!("{}x{}+{}+{}", rect.width, rect.height, rect.x, rect.y))
            .unwrap_or_else(|| "-".to_owned());
        output.push_str(&format!(
            "{}  {}  {}  {}  {}  {}\n",
            window.id.0,
            window.workspace.0,
            window.app_id.as_deref().unwrap_or("-"),
            window.title.as_deref().unwrap_or("-"),
            geometry,
            if window.focused { "*" } else { "" }
        ));
    }
    output
}

fn render_workspaces(workspaces: &[WorkspaceInfo]) -> String {
    if workspaces.is_empty() {
        return "(none)\n".to_owned();
    }

    let mut output = "ID  NAME  ACTIVE  WINDOWS  PROJECT\n".to_owned();
    for workspace in workspaces {
        output.push_str(&format!(
            "{}  {}  {}  {}  {}\n",
            workspace.id.0,
            workspace.name,
            if workspace.active { "*" } else { "" },
            workspace.window_ids.len(),
            workspace.project_path.as_deref().unwrap_or("-")
        ));
    }
    output
}

fn render_outputs(outputs: &[OutputInfo]) -> String {
    if outputs.is_empty() {
        return "(none)\n".to_owned();
    }

    let mut text = "NAME  MODE  SCALE  TRANSFORM  POSITION  MAKE/MODEL\n".to_owned();
    for output in outputs {
        let mode = output
            .mode
            .as_ref()
            .map(|mode| format!("{}x{}@{}", mode.width, mode.height, mode.refresh))
            .unwrap_or_else(|| "-".to_owned());
        text.push_str(&format!(
            "{}  {}  {}  {}  +{}+{}  {} {}\n",
            output.name,
            mode,
            output.scale,
            output.transform,
            output.position.x,
            output.position.y,
            output.make,
            output.model
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixListener, thread};

    use super::*;
    use sayuki_ipc::{PROTOCOL_VERSION, WindowId, WorkspaceId};

    #[test]
    fn spawn_preserves_trailing_argv() {
        let args = Args::try_parse_from([
            "sayukictl",
            "--socket",
            "/tmp/sayuki.sock",
            "spawn",
            "--",
            "ghostty",
            "--class",
            "Sayuki",
        ])
        .expect("args");

        assert_eq!(
            request_from_command(args.command),
            Request::Action {
                action: Action::Spawn {
                    argv: vec![
                        "ghostty".to_owned(),
                        "--class".to_owned(),
                        "Sayuki".to_owned()
                    ],
                },
            }
        );
    }

    #[test]
    fn missing_socket_is_an_error() {
        let error = require_socket_path(None).expect_err("missing socket");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            error.to_string(),
            "SAYUKI_SOCKET is not set; pass --socket <path> or run sayukictl from a Sayuki-spawned process"
        );
    }

    #[test]
    fn query_subcommands_map_to_requests() {
        for (command, request) in [
            ("windows", Request::GetWindows),
            ("workspaces", Request::GetWorkspaces),
            ("outputs", Request::GetOutputs),
            ("focused", Request::GetFocused),
        ] {
            let args = Args::try_parse_from(["sayukictl", "--socket", "/tmp/s.sock", command])
                .expect("args");
            assert_eq!(request_from_command(args.command), request);
        }
    }

    #[test]
    fn json_render_is_machine_readable() {
        let reply = Reply::Focused {
            window: Some(WindowId(3)),
            workspace: WorkspaceId(1),
        };

        assert_eq!(
            serde_json::from_str::<Reply>(&render_reply(&reply, true)).expect("reply"),
            reply
        );
    }

    #[test]
    fn table_render_lists_windows() {
        let text = render_reply(
            &Reply::Windows {
                windows: vec![WindowInfo {
                    id: WindowId(5),
                    app_id: Some("ghostty".to_owned()),
                    title: None,
                    workspace: WorkspaceId(1),
                    floating: true,
                    focused: true,
                    geometry: Some(sayuki_ipc::Rect {
                        x: 0,
                        y: 0,
                        width: 800,
                        height: 600,
                    }),
                }],
            },
            false,
        );

        assert!(text.contains("ID"));
        assert!(text.contains("5"));
        assert!(text.contains("ghostty"));
        assert!(text.contains("800x600+0+0"));
        assert!(render_reply(&Reply::Windows { windows: vec![] }, false).contains("(none)"));
    }
    #[test]
    fn client_round_trips_version_reply() {
        let path = std::env::temp_dir().join(format!(
            "sayukictl-test-{}-{}.sock",
            std::process::id(),
            "version"
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server_path = path.clone();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            let request = loop {
                let read = stream.read(&mut chunk).expect("read");
                assert_ne!(read, 0, "client closed before request");
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(request) = try_decode_frame::<Request>(&mut buffer).expect("request") {
                    break request;
                }
            };
            assert_eq!(request, Request::GetVersion);

            let reply = Reply::Version {
                compositor: "test".to_owned(),
                protocol: PROTOCOL_VERSION,
            };
            let frame = encode_frame(&reply).expect("reply frame");
            stream.write_all(&frame).expect("write reply");
            std::fs::remove_file(server_path).expect("remove socket");
        });

        let reply = request_reply(&path, &Request::GetVersion).expect("reply");
        assert_eq!(
            reply,
            Reply::Version {
                compositor: "test".to_owned(),
                protocol: PROTOCOL_VERSION,
            }
        );
        handle.join().expect("server thread");
    }

    #[test]
    fn client_round_trips_windows_reply() {
        let path = std::env::temp_dir().join(format!(
            "sayukictl-test-{}-{}.sock",
            std::process::id(),
            "windows"
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server_path = path.clone();

        let expected = Reply::Windows {
            windows: vec![WindowInfo {
                id: WindowId(1),
                app_id: Some("ghostty".to_owned()),
                title: Some("zsh".to_owned()),
                workspace: WorkspaceId(1),
                floating: true,
                focused: true,
                geometry: Some(sayuki_ipc::Rect {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                }),
            }],
        };
        let server_reply = expected.clone();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 8192];
            let request = loop {
                let read = stream.read(&mut chunk).expect("read");
                assert_ne!(read, 0, "client closed before request");
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(request) = try_decode_frame::<Request>(&mut buffer).expect("request") {
                    break request;
                }
            };
            assert_eq!(request, Request::GetWindows);

            let frame = encode_frame(&server_reply).expect("reply frame");
            stream.write_all(&frame).expect("write reply");
            std::fs::remove_file(server_path).expect("remove socket");
        });

        let reply = request_reply(&path, &Request::GetWindows).expect("reply");
        assert_eq!(reply, expected);
        handle.join().expect("server thread");
    }
}
