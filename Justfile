set dotenv-load := false

default:
    @just --list

check:
    cargo clippy --workspace --all-targets

fmt:
    cargo fmt --all

flake-check:
    nix flake check

run:
    cargo run -p sayuki-compositor

dev:
    nix develop

ci: fmt check flake-check
