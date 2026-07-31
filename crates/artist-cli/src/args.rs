use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "artist", version, about = "The Artist coding agent")]
pub struct Cli {
    /// Prompt to send immediately, or a project directory to open.
    #[arg(value_name = "PROMPT_OR_PROJECT")]
    pub prompt: Option<String>,
    /// Execute one prompt and print the response without opening the chat UI.
    #[arg(short = 'p', long, value_name = "PROMPT")]
    pub print_prompt: Option<String>,
    /// Resume a session by ID, or select one interactively when no ID is given.
    #[arg(short = 'r', long = "resume", value_name = "SESSION_ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Select the model and reasoning effort for the default provider.
    Model,
    /// Manage stream rules.
    Rules(RulesArgs),
    /// Inspect and maintain stored sessions.
    Sessions(SessionsArgs),
    /// Inspect agent profiles.
    Profiles(ProfilesArgs),
}

#[derive(Debug, Args)]
pub struct ProfilesArgs {
    #[command(subcommand)]
    pub action: ProfilesCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfilesCommand {
    /// List the profiles available in this project and where each comes from.
    List,
    /// Print a built-in profile, ready to save as an override.
    ///
    /// Nothing is written to your config until you do it: redirect this into
    /// .artist/profiles/<name>.md and edit from there.
    Show { name: String },
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
        /// Keep this many recent sessions per project.
        #[arg(long, default_value_t = 10)]
        keep: usize,
        /// Only delete sessions older than this many days.
        #[arg(long, default_value_t = 30)]
        older_than_days: u64,
        /// Show what would be deleted without deleting.
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
    fn parses_requested_forms() {
        let provider = Cli::try_parse_from(["artist", "provider"]).unwrap();
        assert!(provider.command.is_none());
        assert_eq!(provider.prompt.as_deref(), Some("provider"));
        assert!(Cli::try_parse_from(["artist", "model"]).is_ok());
        let cli = Cli::try_parse_from(["artist", "-p", "reply OK"]).unwrap();
        assert_eq!(cli.print_prompt.as_deref(), Some("reply OK"));
        let cli = Cli::try_parse_from(["artist", "-p", "reply OK", "/tmp"]).unwrap();
        assert_eq!(cli.print_prompt.as_deref(), Some("reply OK"));
        assert_eq!(cli.prompt.as_deref(), Some("/tmp"));
        assert_eq!(
            Cli::try_parse_from(["artist", "hello"])
                .unwrap()
                .prompt
                .as_deref(),
            Some("hello")
        );
        let resumed = Cli::try_parse_from(["artist", "hello", "-r", "abc"]).unwrap();
        assert_eq!(resumed.prompt.as_deref(), Some("hello"));
        assert_eq!(resumed.resume.as_deref(), Some("abc"));
        assert_eq!(
            Cli::try_parse_from(["artist", "-r"])
                .unwrap()
                .resume
                .as_deref(),
            Some("")
        );
        assert_eq!(
            Cli::try_parse_from(["artist", "-p", "next", "-r"])
                .unwrap()
                .resume
                .as_deref(),
            Some("")
        );
        assert_eq!(
            Cli::try_parse_from(["artist", "-p", "next", "-r", "abc"])
                .unwrap()
                .resume
                .as_deref(),
            Some("abc")
        );
    }
}
