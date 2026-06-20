use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(author, version, about = "Sayuki nested Wayland compositor")]
pub(crate) struct Args {
    /// Wayland socket name to bind instead of auto-selecting wayland-N.
    #[arg(long)]
    pub(crate) socket: Option<String>,

    /// TOML config file to load for input settings and keybindings.
    #[arg(long)]
    pub(crate) config: Option<PathBuf>,
}
