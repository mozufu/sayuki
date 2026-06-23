use std::path::PathBuf;

use clap::Parser;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum BackendKind {
    Nested,
    Udev,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "Sayuki Wayland compositor")]
pub(crate) struct Args {
    /// Backend to run. `nested` opens a winit window; `udev` runs on DRM/KMS from a TTY.
    #[arg(long, value_enum, default_value_t = BackendKind::Nested)]
    pub(crate) backend: BackendKind,

    /// Wayland socket name to bind instead of auto-selecting wayland-N.
    #[arg(long)]
    pub(crate) socket: Option<String>,

    /// `.zt` config file to load. When omitted, the compositor searches
    /// `$XDG_CONFIG_HOME/sayuki/config.zt` then `/etc/sayuki/config.zt`.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Args, BackendKind};

    #[test]
    fn backend_selector_accepts_udev() {
        let args = Args::try_parse_from(["sayuki-compositor", "--backend", "udev"])
            .expect("udev backend selector should parse");
        assert_eq!(args.backend, BackendKind::Udev);

        assert!(Args::try_parse_from(["sayuki-compositor", "--backend", "native"]).is_err());
    }
}
