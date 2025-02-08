use anyhow::Result;
use clap::Parser;
use slkrd::{
    config::Config,
    transfer::Transfer,
};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Send mode with file path
    #[arg(short = 's')]
    send: Option<PathBuf>,

    /// Receive mode with passcode
    #[arg(short = 'r')]
    receive: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let config = Config::default();

    match (args.send, args.receive) {
        (Some(path), None) => {
            let file_size = std::fs::metadata(&path)?.len();
            let _transfer = Transfer::new_sender(
                path,
                config.chunk_size,
                file_size,
            )
            .await?;
            // Handle sending
        }
        (None, Some(passcode)) => {
            // Handle receiving
            println!("Waiting for connection with passcode: {}", passcode);
        }
        _ => {
            eprintln!("Please specify either -s for send or -r for receive mode");
            std::process::exit(1);
        }
    }

    Ok(())
}
