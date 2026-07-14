use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "artist", version, about = "The Artist coding agent")]
pub struct Cli {
    /// Execute one prompt and print the response.
    #[arg(short = 'p', long, value_name = "PROMPT")]
    pub prompt: Option<String>,
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
        assert_eq!(cli.prompt.as_deref(), Some("reply OK"));
        for action in ["list", "set", "test"] {
            assert!(Cli::try_parse_from(["artist", "provider", action]).is_ok());
        }
    }
}
