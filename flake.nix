{
  description = "Sayuki Rust monorepo with a Wayland compositor development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [
            "clippy"
            "rust-src"
            "rustfmt"
          ];
        };

        nativeBuildInputs = with pkgs; [
          pkg-config
          rustToolchain
          rust-analyzer
        ];

        waylandBuildInputs = with pkgs; [
          libGL
          libdisplay-info_0_2
          libdrm
          libinput
          libxkbcommon
          mesa
          libgbm
          pixman
          seatd
          udev
          vulkan-loader
          wayland
          wayland-protocols
          wayland-scanner
        ];

        formatter = pkgs.writeShellApplication {
          name = "sayuki-format";
          runtimeInputs = with pkgs; [
            findutils
            nixfmt
          ];
          text = ''
            if [ "$#" -eq 0 ]; then
              set -- flake.nix
            fi

            for path in "$@"; do
              if [ -d "$path" ]; then
                if [ ! -w "$path" ]; then
                  path="$PWD"
                fi
                find "$path" -name '*.nix' -not -path '*/.direnv/*' -print0 | xargs -0 -r nixfmt
              else
                if [ ! -w "$path" ] && [ -f "$PWD/$(basename "$path")" ]; then
                  path="$PWD/$(basename "$path")"
                fi
                nixfmt "$path"
              fi
            done
          '';
        };
      in
      {
        devShells.default = pkgs.mkShell {
          packages =
            nativeBuildInputs
            ++ waylandBuildInputs
            ++ (with pkgs; [
              bacon
              cargo-edit
              cargo-nextest
              cargo-watch
              mold
              nixfmt
            ]);

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath waylandBuildInputs;
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            export RUST_BACKTRACE=1
            export RUSTFLAGS="-C link-arg=-fuse-ld=mold ''${RUSTFLAGS:-}"
            echo "Sayuki Wayland compositor development shell"
          '';
        };

        formatter = formatter;
      }
    );
}
