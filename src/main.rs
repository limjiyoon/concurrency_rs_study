mod echo_server;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command_group: CommandGroup,
}

#[derive(Subcommand)]
enum CommandGroup {
    /// Test Async echo server
    ASYNC {
        #[command(subcommand)]
        command: AsyncCommand,
    },
    /// Test Lock algorithm
    LOCK,
}

#[derive(Subcommand)]
enum AsyncCommand {
    /// Test MIO echo server
    Mio,
    /// Test MIO echo server with Future trait
    MioFuture,
    ///Test Tokio echo server
    Tokio,
}

fn main() {
    let cli = Cli::parse();
    match &cli.command_group {
        CommandGroup::ASYNC { command } => match command {
            AsyncCommand::Mio => {
                echo_server::mio_echo_server::run();
            }
            AsyncCommand::MioFuture => {
                echo_server::mio_future_server::run();
            }
            AsyncCommand::Tokio => {
                echo_server::tokio_echo_server::run();
            }
        },
        CommandGroup::LOCK => {
            println!("Lock");
        }
    }
}
