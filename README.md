# Sayuki

Sayuki is a Rust workspace for Wayland compositor development.

The repository currently has one binary crate:

- `crates/sayuki-compositor`: nested Smithay/winit compositor executable.

## Toolchain

Sayuki uses Rust 2024 edition and tracks the latest stable Rust toolchain through
the Nix development shell.

Required tools:

- Nix with flakes enabled
- direnv, optional but recommended

The flake provides Rust, Cargo, Clippy, rustfmt, rust-analyzer, and common
Wayland compositor development libraries.

## Development

Enter the development environment with direnv:

```sh
direnv allow
```

Or enter the shell directly:

```sh
nix develop
```

Useful commands:

```sh
cargo clippy --workspace --all-targets
cargo fmt --all
nix flake check
```

Run the nested compositor:

```sh
cargo run -p sayuki-compositor
```

The compositor prints the selected Wayland socket. Start test clients with that
socket name, for example `WAYLAND_DISPLAY=wayland-1 <client>`.

## Workspace

```text
.
|-- Cargo.toml
|-- flake.nix
`-- crates/
    `-- sayuki-compositor/
```

Workspace-wide package metadata and lints live in the root `Cargo.toml`.
