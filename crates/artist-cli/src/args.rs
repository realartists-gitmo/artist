use clap::{Args, Parser, Subcommand, ValueEnum};

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
    Provider(ProviderArgs),
    /// Select the model and reasoning effort for the default provider.
    Model,
}

#[derive(Debug, Args)]
pub struct ProviderArgs {
    #[arg(long, value_enum)]
    pub login: Option<LoginKind>,
    #[command(subcommand)]
    pub action: Option<ProviderAction>,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum LoginKind {
    Chatgpt,
}

#[derive(Debug, Subcommand)]
pub enum ProviderAction {
    List,
    Set,
    Test,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_requested_forms() {
        assert!(Cli::try_parse_from(["artist", "provider", "--login", "chatgpt"]).is_ok());
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
        for action in ["list", "set", "test"] {
            assert!(Cli::try_parse_from(["artist", "provider", action]).is_ok());
        }
    }
}
