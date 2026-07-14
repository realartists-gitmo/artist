use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "artist", version, about = "The Artist coding agent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
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
        for action in ["list", "set", "test"] {
            assert!(Cli::try_parse_from(["artist", "provider", action]).is_ok());
        }
    }
}
