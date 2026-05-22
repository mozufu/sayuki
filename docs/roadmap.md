# Sayuki Smithay Roadmap

Sayuki is planned as a Smithay-based Wayland compositor. The early goal is to
make the compositor easy to iterate on in a nested session before investing in a
full DRM/GBM/libinput backend.

## Milestones

### 1. Nested Smithay compositor ✅

Build the first usable compositor against Smithay's nested `winit` backend.

Status: complete in `crates/sayuki-compositor`.

- [x] initialize the Smithay display and `calloop` event loop
- [x] create and advertise a Wayland socket
- [x] define the root compositor state type
- [x] create one logical output
- [x] render a simple background
- [x] wire basic seat, keyboard, and pointer state
- [x] accept simple Wayland clients

This should be the first implementation target because it can run under an
existing desktop session.

### 2. Basic `xdg-shell` window management

Make regular desktop clients appear and be controllable.

- support `xdg-shell` toplevel surfaces
- map/unmap windows into a Smithay `Space`
- handle frame callbacks
- implement pointer and keyboard focus
- support click-to-focus
- support interactive move and resize
- keep a simple floating stack

### 3. Input and compositor actions

Make the compositor comfortable enough for daily development testing.

- load an xkb keymap
- track modifiers
- define compositor actions such as quit, spawn terminal, move/resize, and
  workspace switching
- add configurable keybindings
- support pointer motion, buttons, axis events, and cursor images

### 4. Real hardware backend

After the nested backend is usable, add the native backend for running from a
TTY.

- discover DRM devices through udev
- initialize GBM/EGL/GLES rendering
- consume libinput events
- integrate session handling through libseat
- support output hotplug and modesetting
- handle VT switch pause/resume

### 5. Window manager model

Move from "example compositor" behavior to Sayuki's own policy.

- workspaces
- output assignment
- focus stack
- floating windows
- tiling layouts, if desired
- window rules
- per-output scale and transform policy

### 6. Desktop protocols and polish

Add protocols as the compositor needs them.

- `xdg-output`
- `xdg-decoration`
- layer shell for panels, backgrounds, and notifications
- data device and clipboard
- primary selection
- viewporter
- fractional scale
- presentation time
- idle inhibit
- screencopy later
- XWayland much later

### 7. Configuration and IPC

Make Sayuki scriptable and configurable.

- parse a config file, likely TOML at first
- configure keybindings, outputs, and window rules
- support live reload where safe
- expose a Unix socket IPC protocol
- add an optional `sayukictl` binary

## Workspace crate plan

Start inside `crates/sayuki-compositor` as modules. Split crates only after the
interfaces are clear.

Likely future crates:

- `sayuki-compositor`: main binary and top-level backend selection
- `sayuki-core`: Smithay state, protocol glue, event loop integration
- `sayuki-wm`: windows, workspaces, focus, stacking, layout policy
- `sayuki-input`: keybindings, input actions, xkb helpers
- `sayuki-config`: config data model, parsing, validation
- `sayuki-ipc`: IPC message types and server/client helpers
- `sayukictl`: optional command-line IPC client

Possible later crates:

- `sayuki-render`: rendering helpers, decorations, damage helpers
- `sayuki-backend`: abstraction over nested, X11, and DRM/udev backends if the
  backend code grows large
- `sayuki-protocols`: generated bindings for custom protocol XML, if any

## Initial dependency policy

The root manifest owns shared dependency versions. Individual crates should use
`workspace = true` where possible.

The first dependency set is intentionally focused on the nested compositor:

- `smithay` with `backend_winit`, `wayland_frontend`, and `desktop`
- `calloop` for the event loop
- `tracing` and `tracing-subscriber` for logging
- `snafu` for structured error handling
- `clap` for command-line flags
- `serde` and `toml` for the future config file
- `bitflags` for compositor state flags

Add heavier Smithay features such as `backend_drm`, `backend_gbm`,
`backend_libinput`, `backend_udev`, `backend_session_libseat`,
`renderer_multi`, and `renderer_pixman` when the native backend milestone starts.
