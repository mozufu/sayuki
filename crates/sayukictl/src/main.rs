use clap::{Parser, Subcommand};
use sayuki_ipc::Request;

#[derive(Debug, Parser)]
#[command(version, about = "Control a running Sayuki compositor")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build a health-check IPC request.
    Ping,
    /// Build an action IPC request.
    RunAction { action: String },
}

fn main() {
    let args = Args::parse();
    let request = match args.command {
        Command::Ping => Request::Ping,
        Command::RunAction { action } => Request::RunAction { action },
    };

    println!("{request:?}");
}
