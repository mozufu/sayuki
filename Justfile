set dotenv-load := false

compositor := "sayuki-compositor"

# List available recipes.
default:
    @just --list

# Enter the Nix development shell.
dev:
    nix develop

# Run an arbitrary command inside the Nix development shell.
nix *cmd:
    nix develop -c {{cmd}}

# Build the workspace.
build:
    cargo build --workspace

# Run workspace tests.
test:
    cargo test --workspace --all-targets

# Run Clippy for the workspace.
check:
    cargo clippy --workspace --all-targets

# Format all Rust code.
fmt:
    cargo fmt --all

# Check Rust formatting without writing changes.
fmt-check:
    cargo fmt --all -- --check

# Run the Nix flake checks.
flake-check:
    nix flake check

# Run the nested compositor for interactive/manual testing.
test-compositor *args:
    cargo run -p {{compositor}} -- {{args}}

# Run the compositor on real hardware/TTY via the udev backend.
run *args:
    cargo run -p {{compositor}} -- --backend udev {{args}}

# Remove build artifacts.
clean:
    cargo clean

# Run the standard local validation suite.
ci: fmt-check check test flake-check
