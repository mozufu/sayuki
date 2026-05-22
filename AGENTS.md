# Agent Instructions

## Project Overview

Sayuki is a Wayland compositor written in Rust.
The Nix flake is the source of truth for the development toolchain 
and native Wayland dependencies.

## Environment

Use the Nix shell for project commands:

```sh
nix develop
```

The shell tracks latest stable Rust through `oxalica/rust-overlay` and includes
Cargo, Clippy, rustfmt, rust-analyzer, pkg-config, and Wayland-related native
libraries.

## Common Commands

Run these from the repository root:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
nix flake check
```

Use `cargo run -p sayuki-compositor` to run the starter binary.

## Repository Layout

- `Cargo.toml`: workspace membership, shared package metadata, and lints.
- `flake.nix`: development shell and formatter configuration.
- `crates/sayuki-compositor`: starter binary crate for compositor work.

## Coding Guidelines

- Keep crates on the workspace Rust edition and shared lint configuration.
- Prefer workspace-level dependency and lint configuration when adding crates.
- Keep compositor-specific code inside `crates/sayuki-compositor` until a clear
  shared abstraction is needed.
- Do not bypass the Nix shell when validating changes that depend on native
  libraries.
- Avoid committing generated build artifacts or local environment files.
- Use conventional commit messages, such as `feat: add input handling` or
  `fix: handle empty device list`.

## Validation Expectations

For normal code changes, run `cargo clippy --workspace --all-targets` as the
default check. For changes touching formatting, lint settings, Nix, or
dependency/toolchain setup, also run the relevant command from the common command
list above.
