# Sayuki Smithay Roadmap

Sayuki is planned as a Smithay-based Wayland compositor. The early goal is to
make the compositor easy to iterate on in a nested session before investing in a
full DRM/GBM/libinput backend.

Guiding shortcut: prefer proven compositor building blocks over custom plumbing.
Start from Smithay's high-level APIs and example patterns, especially `anvil`,
then borrow policy ideas from mature compositors such as `niri`, `cosmic-comp`,
`sway`, `river`, and `dwl`. Only drop to raw Wayland protocol handling or custom
rendering when Smithay does not already provide the needed abstraction.

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

### 2. Basic `xdg-shell` window management ✅

Make regular desktop clients appear and be controllable.

Status: complete in `crates/sayuki-compositor`.

- [x] support `xdg-shell` toplevel surfaces
- [x] map/unmap windows into a Smithay `Space`
- [x] handle frame callbacks
- [x] implement pointer and keyboard focus
- [x] support click-to-focus
- [x] support interactive move and resize
- [x] keep a simple floating stack

### 3. Input and compositor actions

Make the compositor comfortable enough for daily development testing.

Status: complete in `crates/sayuki-compositor`. The nested compositor forwards
normal keyboard and pointer input, supports configurable keyboard settings and
keybindings, runs compositor actions, and has a workspace action placeholder for
Milestone 5.

- [x] track modifiers through Smithay's keyboard input path
- [x] add a first compositor action: `Alt+Enter` spawns `ghostty`
- [x] suppress compositor-handled key presses so clients do not also receive them
- [x] support pointer motion, buttons, and axis events
- [x] load a configurable xkb keymap
- [x] replace the hard-coded key daemon with an action/keybinding registry
- [x] define compositor actions such as quit, spawn command, move/resize, and
  workspace switching
- [x] add configurable keybindings
- [x] support client-provided cursor images

### 4. Real hardware backend

Status: complete in crates/sayuki-compositor

After the nested backend is usable, add the native backend for running from a
TTY. Shortcut: port the shape of Smithay `anvil`'s udev/DRM/libinput backend
first, then abstract only the parts Sayuki actually needs to differ.

- [x] discover DRM devices through udev
- [x] initialize GBM/EGL/GLES rendering
- [x] consume libinput events
- [x] integrate session handling through libseat
- [x] support output hotplug and modesetting
- [x] handle VT switch pause/resume

### 5. Window manager model

Move from "example compositor" behavior to Sayuki's own policy: a **viewport
over an unbounded canvas**, oriented around projects. Windows float at free,
persistent canvas coordinates; monitors are viewports on the canvas; switching
project/workspace swaps the canvas. A workspace is a project context (a named
canvas), not a numeric slot — carrying a working directory, an environment (via
direnv), and a desktop session (apps, layout, window rules). direnv owns the
environment; Sayuki owns the windows and session.

See `docs/milestone-5-window-manager-model.md` for the detailed spec, split into
5a (canvas/viewport WM core / mechanism) and 5b (project session layer / policy).

5a — canvas/viewport WM core (✅ complete in `crates/sayuki-compositor`):

- viewport over an unbounded canvas; windows at free, persistent coordinates,
  no clamping to outputs
- a canvas per project/workspace, each its own Smithay `Space`; switching swaps
  the canvas (cameras move, windows stay)
- multiple monitors as viewports on the shared canvas, independently
  pannable/zoomable (configurable: linked)
- per-canvas focus stacks with reveal-on-focus
- pin-to-viewport (sticky HUD) windows
- snap-on-drag and window swap
- navigation: zoom, overview (fit-all), minimap

5b — project session layer (✅ complete in `crates/sayuki-compositor`):

- project context per canvas: working directory, env overlay, lifecycle hooks
- direnv integration for spawned processes (`direnv exec`, non-interactive)
- `.sayuki` project files plus central `[[project]]` config, with a trust gate
- window rules
- per-output scale and transform policy
- tiling layouts (deferred to milestone 8)

### 6. Desktop protocols and polish

Add protocols as the compositor needs them. Prefer Smithay protocol handlers and
helpers before adding generated protocol glue directly. Two consumers, two
transports: a first-party shell over Sayuki IPC (milestone 7), and the existing
Wayland ecosystem (waybar, grim, wl-clipboard, swayidle) over standard wlr/ext
protocols. Build both.

See `docs/milestone-6-desktop-protocols.md` for the detailed spec (priority
tiers, the layer-shell work-area contract, and Smithay handler mapping).

Tier 0 — shell foundation (✅ complete in `crates/sayuki-compositor`):

- layer shell for panels, backgrounds, notifications, launchers, lock surfaces
- `xdg-output`

Tier 1 — ecosystem interop (✅ complete in `crates/sayuki-compositor`):

- [x] `ext-foreign-toplevel-list` — external taskbars/docks enumerate windows
- [x] `wlr-data-control` + `ext-data-control` clipboard for managers
  (`wl-clipboard`, `cliphist`), beyond the existing basic data-device plumbing
- foreign-toplevel **management** and screencopy / image-copy-capture deferred
  to milestone 8

Tier 2 — session and security (✅ complete in `crates/sayuki-compositor`):

- [x] session lock
- [x] idle notify and idle inhibit
- [x] security context (sandboxed client socket registration; enforcement deferred to M7)

Tier 3 — input completeness (✅ complete in `crates/sayuki-compositor`):

- [x] text-input/input-method (IME) and virtual keyboard
- [x] pointer constraints and relative pointer
- [x] cursor shape
- [x] `xdg-activation`

Tier 4 — visual polish (✅ complete in `crates/sayuki-compositor`):

- [x] `xdg-decoration` (server-side default, CSD opt-in)
- [x] fractional scale and viewporter
- [x] presentation time
- [x] primary selection

### 7. Configuration and IPC

Make Sayuki scriptable and configurable. IPC is the DE control plane: one
`Action` seam shared by keybindings and IPC, a shared `sayuki-ipc` crate, and a
`sayukictl` client. The config file is the source of truth with hot reload; IPC
issues ephemeral actions.

See `docs/milestone-7-config-and-ipc.md` for the detailed spec (wire format,
`sayuki-ipc` type sketch, command/query/event taxonomy, hot-reload safety
matrix).

- [x] shared `sayuki-ipc` crate: wire types, frame codec, `Action`/`Request`/`Reply`
- [x] `sayukictl` client with `version`, `quit`, `spawn`, `windows`, `workspaces`,
  `outputs`, `focused` subcommands and `--json` output
- [x] Unix socket IPC: request/reply transport integrated into the calloop event loop
- [x] query read path: `GetWindows`, `GetWorkspaces`, `GetOutputs`, `GetFocused`
  return live compositor snapshots; stable `WindowId` stored in window user-data
- [x] layered config: defaults (`/etc/sayuki/config.zt`), user (`$XDG_CONFIG_HOME/sayuki/config.zt`),
  fallback to Rust built-in defaults; per-project `.sayuki` files evaluated via `eval_with_base`
- [x] configure keyboard, input, keybindings, outputs, and projects via `.zt` config files;
  typed `Action` tagged union replaces stringly-typed keybinding actions
- [x] live reload with atomic validate-then-swap: inotify watches the config file's parent
  directory; a background thread delivers triggers via `calloop::channel`; on success all live
  config is swapped (keyboard XKB + repeat, keybindings, pan/snap policy, output policies);
  on error the compositor keeps running with the previous config unchanged
- subscribable event stream (deferred to milestone 8)

Status: complete except for the event stream, which moves to milestone 8.

### 8. Deferred completions

Items carried forward from earlier milestones where the prerequisite (Smithay
protocol handler, render-to-buffer glue, or design clarity) was not yet in place.
Implement in dependency order: event stream first (unblocks live panels), then
screencopy (unblocks `grim` and the xdg-desktop-portal path), then
foreign-toplevel management and tiling.

- [x] **Subscribable event stream** (from M7) — `Subscribe(Vec<EventKind>)`
  request upgrades a connection to an event stream; compositor emits
  `Event::Window*`, `Event::Workspace*`, `Event::Output*`, `Event::Config*` as
  side effects of state mutations; backpressure: slow subscriber is dropped;
  events serialized once and shared across all subscribers. Emitted from
  `SayukiState` mutations (window open/close/focus, workspace focus, config
  reload/error, nested-output config); udev output hotplug routes through
  backend free functions without subscriber access, so those `OutputChanged`
  emissions remain a documented gap.
- [x] **Screencopy / image-copy-capture** (from M6 Tier 1) — hand-written
  `wlr-screencopy-unstable-v1` (manager v3) glue in `screencopy.rs`; Smithay 0.7
  ships no helper, so the module implements the `GlobalDispatch`/`Dispatch` impls
  directly. Both backends render through `GlesRenderer`, so a single `ExportMem`
  offscreen-readback path serves both: captures are deferred onto
  `SayukiState::pending_screencopy` and drained by `fulfill_screencopy` after the
  on-screen render (wlr "next frame" semantics). SHM only (`Xrgb8888`), rendered
  upright; `grim` capture verified end-to-end (full-output PNG, pixels match the
  background). `copy_with_damage` reports full-frame damage every frame (the
  compositor has no damage-tracking subsystem yet) — a valid superset, but the
  incremental-capture optimisation remains a documented gap. Foundation for the
  xdg-desktop-portal ScreenCast backend.
- [x] **Foreign-toplevel management** (from M6 Tier 1) — hand-written
  `wlr-foreign-toplevel-management` (manager v3) glue in `foreign_toplevel.rs`;
  Smithay 0.7 ships only the *list* helper, so the module implements the
  `GlobalDispatch`/`Dispatch` impls directly (screencopy precedent), reusing the
  existing WM paths: `activate` → focus/raise (switching to the window's canvas
  first when it is off-screen), `close` → `xdg_toplevel.close`, and
  set/unset fullscreen + maximize → the existing `xdg-shell` request handlers.
  The `state` array reflects committed xdg maximized/fullscreen plus a synthetic
  `activated` bit for the keyboard-focused window, kept in sync across every
  focus path (click, layer/lock focus, close). Per-client manager instances each
  get their own handles, with `output_enter`/`leave` tracking the window's
  current output (best-effort, reconverging on commit). `waybar`/taskbar can
  enumerate, raise, and close windows. Minimize and `set_rectangle` are
  intentional no-ops (the canvas model has no minimize); the `parent` event is
  never emitted.
- [ ] **Tiling layouts** (from M5b) — first-class tiling alongside the floating
  canvas model; snap/swap remains the floating-first default; tiling is
  opt-in per workspace or via window rules; reference `niri`'s column layout and
  `river`'s layout protocol for the policy shape.

Far: XWayland (legacy X11 apps; large integration; defer until native clients are
solid); xdg-desktop-portal backend (`xdg-desktop-portal-sayuki` — separate DBus
service implementing ScreenCast via PipeWire, Screenshot, Settings,
GlobalShortcuts; builds on M8 screencopy).

### 9. First-party protocols (`sayuki-protocols`)

Sayuki's own Wayland protocol extensions for the project-oriented WM concepts that
no standard protocol expresses and the out-of-band IPC plane cannot reach: a
capability earns a custom protocol only when it is bound to `wl_surface`/`wl_client`
lifecycle **and** no standard wlr/ext protocol covers it. Everything else stays in
`sayuki-ipc` (control plane) or standard protocols (ecosystem interop).

See `docs/milestone-9-first-party-protocols.md` for the detailed spec (interface
sketches, the security-context trust gate, the shared M8 emission seam, and the
crate/build plumbing).

- [ ] **`sayuki-protocols` crate** — `protocols/*.xml` + `build.rs` running
  `wayland-scanner`; generated bindings only, with `GlobalDispatch`/`Dispatch`
  impls in the compositor (the hand-rolled `screencopy.rs` pattern).
- [ ] **Security-context trust gate** (finishes M6 Tier 2 enforcement) — replace
  the `|_| true` filter with a real per-client trust predicate; advertise the
  `zsayuki_*` globals only to trusted first-party clients, withhold from sandboxes.
- [ ] **`zsayuki_project_v1`** — per-surface project affinity: a client tags an
  `xdg_toplevel` with project name + persistent canvas coords + rule hints before
  map, so placement is race-free instead of heuristic; closes the M7 gap for
  externally-spawned windows.
- [ ] **`zsayuki_canvas_v1`** — the unbounded-canvas viewport, per `wl_output`:
  observer events (viewport, window geometry, project change) for minimap/overview
  clients, and controller requests (pan/zoom/focus/overview) for the shell, fed
  from and routed through the same seams IPC already uses.

Then (`sayuki-ipc`, not protocols): project-lifecycle `Action`s (create/switch/
close, apply layout, run hooks) for a first-party project shell.

Deferred: frame-synced minimap rendering; incremental geometry diffs; strict
unknown-project errors.

## Reference-first development policy

Before implementing a sizeable compositor feature, check whether Smithay already
has a helper, handler, render element, or example implementation. Good default
sources to inspect:

- Smithay `anvil` for backend setup, event loop integration, protocol handlers,
  grabs, and output management
- `niri` and `cosmic-comp` for production Smithay architecture
- `sway`, `river`, and `dwl` for window-management policy and configuration UX

Prefer adapting the smallest useful piece over designing a general abstraction in
advance.

## Workspace crate plan

The workspace globs in every crate directory (`members = ["crates/*"]`). The
original plan — *write logic as modules inside `sayuki-compositor`, split crates
only once interfaces are clear* — still holds; in practice the split has been
**scaffolded ahead of extraction**: the target crate directories exist with a
documented `lib.rs`, but most compositor logic still lives as modules in
`sayuki-compositor` and the boundaries are extracted incrementally.

Current crates and status:

| Crate | Status | Role |
|---|---|---|
| `sayuki-compositor` | populated (binary) | main binary; holds the live state, WM, input, config, render, IPC server, and protocol glue as modules (`state.rs`, `wm/`, `input/`, `config.rs`, `render.rs`, `ipc.rs`, `screencopy.rs`, …) pending extraction |
| `sayuki-ipc` | populated, in use | wire types + frame codec for the Unix-socket control plane; depended on by the compositor and `sayukictl` |
| `sayukictl` | populated, in use | command-line IPC client |
| `sayuki-core` | scaffolding | shared runtime primitives (app id, component metadata); target home for Smithay state / event-loop glue and the M9 `zsayuki_*` `Dispatch` impls |
| `sayuki-wm` | scaffolding | window/workspace/focus/stacking/layout policy types for the canvas model |
| `sayuki-input` | scaffolding | keybinding, input-action, and xkb policy primitives |
| `sayuki-config` | scaffolding | config data model, parsing, validation |
| `sayuki-render` | scaffolding | rendering helpers: decorations, damage, effects |

"Scaffolding" means the crate exists with a doc-commented `lib.rs` but the
corresponding logic has not yet been moved out of `sayuki-compositor`; extraction
happens as each interface stabilizes.

Committed, not yet created:

- `sayuki-protocols`: generated bindings for Sayuki's custom protocol XML
  (`zsayuki_project_v1`, `zsayuki_canvas_v1`); created in milestone 9. Holds
  generated bindings only — `GlobalDispatch`/`Dispatch` impls stay with the
  compositor / `sayuki-core`.

Possible later crate:

- `sayuki-backend`: abstraction over nested, X11, and DRM/udev backends if the
  backend code grows large.

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
