use std::path::PathBuf;

use anyhow::Context;
use artist_cli::{ToolRunner, validate_root};
use clap::{Parser, Subcommand};
use tokio::io::AsyncReadExt;

#[derive(Debug, Parser)]
#[command(name = "artist", about = "Drive Artist tools and services")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Invoke a default verb with a TOON request from a file or stdin.
    Tool {
        /// Verb name, for example read, edit, find, run, or signal.
        verb: String,
        /// TOON input file. Omit it to read stdin.
        #[arg(short, long)]
        input: Option<PathBuf>,
        /// Host directory exposed as files:///
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    match args.command {
        Command::Tool { verb, input, root } => {
            validate_root(&root)?;
            let payload = match input {
                Some(path) => tokio::fs::read_to_string(path)
                    .await
                    .context("read TOON input")?,
                None => {
                    let mut payload = String::new();
                    tokio::io::stdin()
                        .read_to_string(&mut payload)
                        .await
                        .context("read TOON input from stdin")?;
                    payload
                }
            };
            let runner = ToolRunner::new(root)?;
            let (output, ok) = runner.call_toon(&verb, &payload).await?;
            println!("{output}");
            if !ok {
                anyhow::bail!("tool call failed")
            }
        }
    }
    Ok(())
}
