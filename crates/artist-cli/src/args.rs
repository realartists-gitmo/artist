use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "artist",
    version,
    about = "Artist maintenance utilities (model runtime temporarily removed)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage stream rules.
    Rules(RulesArgs),
    /// Inspect and maintain stored sessions.
    Sessions(SessionsArgs),
}

#[derive(Debug, Args)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub action: SessionsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List sessions for the current project with on-disk sizes.
    List,
    /// Regenerate a session's markdown transcript from its event log.
    Render { id: String },
    /// Delete old sessions (never the N most recent per project).
    Gc {
        #[arg(long, default_value_t = 10)]
        keep: usize,
        #[arg(long, default_value_t = 30)]
        older_than_days: u64,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct RulesArgs {
    #[command(subcommand)]
    pub action: RulesCommand,
}

#[derive(Debug, Subcommand)]
pub enum RulesCommand {
    /// Scaffold a new declarative rule in .artist/rules/.
    New { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_maintenance_commands_are_exposed() {
        assert!(Cli::try_parse_from(["artist", "sessions", "list"]).is_ok());
        assert!(Cli::try_parse_from(["artist", "rules", "new", "example"]).is_ok());
        for removed in ["provider", "model", "login"] {
            assert!(Cli::try_parse_from(["artist", removed]).is_err());
        }
        assert!(Cli::try_parse_from(["artist", "hello"]).is_err());
        assert!(Cli::try_parse_from(["artist", "-p", "hello"]).is_err());
    }
}
