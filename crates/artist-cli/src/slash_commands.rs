//! Domain types for commands entered in the interactive prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub usage: &'static str,
}

pub(crate) static COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "/model",
        description: "Select a model and reasoning effort",
        usage: "/model [model] [reasoning]",
    },
    SlashCommand {
        name: "/statusbar",
        description: "Configure status bar items",
        usage: "/statusbar",
    },
    SlashCommand {
        name: "/skills",
        description: "List available Agent Skills",
        usage: "/skills",
    },
    SlashCommand {
        name: "/tools",
        description: "Enable or disable agent tools",
        usage: "/tools",
    },
    SlashCommand {
        name: "/mcp",
        description: "Manage MCP servers",
        usage: "/mcp [status|start|stop|restart|refresh] [server]",
    },
    SlashCommand {
        name: "/rewind",
        description: "Rewind to an earlier turn, or fork the session there",
        usage: "/rewind [n] [fork]",
    },
    SlashCommand {
        name: "/rules",
        description: "Stream rules: list, scan the session, dry-run, toggle",
        usage: "/rules [scan|dry-run <file>|enable <rule>|disable <rule>]",
    },
    SlashCommand {
        name: "/new",
        description: "Start a fresh session (the current one is kept)",
        usage: "/new",
    },
    SlashCommand {
        name: "/sessions",
        description: "List this project's sessions",
        usage: "/sessions",
    },
    SlashCommand {
        name: "/resume",
        description: "Switch to another session by id",
        usage: "/resume [id]",
    },
    SlashCommand {
        name: "/accounts",
        description: "List logged-in accounts, or switch to one by id",
        usage: "/accounts [id]",
    },
    SlashCommand {
        name: "/login",
        description: "Log in to another ChatGPT account",
        usage: "/login",
    },
    SlashCommand {
        name: "/help",
        description: "Show available commands",
        usage: "/help",
    },
    SlashCommand {
        name: "/quit",
        description: "Exit artist (or press esc / ctrl+c on an empty prompt)",
        usage: "/quit",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParsedCommand<'a> {
    Help,
    Quit,
    Skills,
    Tools,
    StatusBar,
    Mcp {
        action: &'a str,
        server: Option<&'a str>,
    },
    Model {
        model: Option<&'a str>,
        reasoning: Option<&'a str>,
    },
    Rewind {
        target: Option<usize>,
        fork: bool,
    },
    Rules(RulesAction<'a>),
    /// Start a fresh session.
    New,
    /// List this project's sessions.
    Sessions,
    /// Switch to another session; without an id, list the candidates.
    Resume {
        id: Option<&'a str>,
    },
    /// List logged-in accounts, or switch to one by id.
    Accounts {
        id: Option<&'a str>,
    },
    /// Log in to another account.
    Login,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RulesAction<'a> {
    List,
    Scan,
    DryRun { file: &'a str },
    Enable { rule: &'a str },
    Disable { rule: &'a str },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParseError<'a> {
    UnknownCommand(&'a str),
    InvalidUsage {
        command: &'a str,
        usage: &'static str,
    },
}

/// Parses a complete slash command. Non-command input returns `None`.
pub(crate) fn parse(input: &str) -> Option<Result<ParsedCommand<'_>, ParseError<'_>>> {
    let mut words = input.split_whitespace();
    let command = words.next()?;
    if !command.starts_with('/') {
        return None;
    }
    let arguments: Vec<_> = words.collect();
    Some(match (command, arguments.as_slice()) {
        ("/help", []) => Ok(ParsedCommand::Help),
        ("/quit", []) => Ok(ParsedCommand::Quit),
        ("/quit", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/quit",
        }),
        ("/mcp", []) | ("/mcp", ["status"]) => Ok(ParsedCommand::Mcp {
            action: "status",
            server: None,
        }),
        ("/mcp", [action, server])
            if matches!(*action, "start" | "stop" | "restart" | "refresh") =>
        {
            Ok(ParsedCommand::Mcp {
                action,
                server: Some(server),
            })
        }
        ("/mcp", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/mcp [status|start|stop|restart|refresh] [server]",
        }),
        ("/skills", []) => Ok(ParsedCommand::Skills),
        ("/tools", []) => Ok(ParsedCommand::Tools),
        ("/statusbar", []) => Ok(ParsedCommand::StatusBar),
        ("/statusbar", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/statusbar",
        }),
        ("/skills", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/skills",
        }),
        ("/tools", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/tools",
        }),
        ("/help", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/help",
        }),
        ("/model", []) => Ok(ParsedCommand::Model {
            model: None,
            reasoning: None,
        }),
        ("/model", [model]) => Ok(ParsedCommand::Model {
            model: Some(model),
            reasoning: None,
        }),
        ("/model", [model, reasoning]) => Ok(ParsedCommand::Model {
            model: Some(model),
            reasoning: Some(reasoning),
        }),
        ("/model", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/model [model] [reasoning]",
        }),
        ("/rewind", []) => Ok(ParsedCommand::Rewind {
            target: None,
            fork: false,
        }),
        ("/rewind", [n]) if n.parse::<usize>().is_ok() => Ok(ParsedCommand::Rewind {
            target: n.parse().ok(),
            fork: false,
        }),
        ("/rewind", [n, "fork"]) if n.parse::<usize>().is_ok() => Ok(ParsedCommand::Rewind {
            target: n.parse().ok(),
            fork: true,
        }),
        ("/rewind", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/rewind [n] [fork]",
        }),
        ("/rules", []) => Ok(ParsedCommand::Rules(RulesAction::List)),
        ("/rules", ["scan"]) => Ok(ParsedCommand::Rules(RulesAction::Scan)),
        ("/rules", ["dry-run", file]) => Ok(ParsedCommand::Rules(RulesAction::DryRun { file })),
        ("/rules", ["enable", rule]) => Ok(ParsedCommand::Rules(RulesAction::Enable { rule })),
        ("/rules", ["disable", rule]) => Ok(ParsedCommand::Rules(RulesAction::Disable { rule })),
        ("/rules", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/rules [scan|dry-run <file>|enable <rule>|disable <rule>]",
        }),
        ("/new", []) => Ok(ParsedCommand::New),
        ("/new", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/new",
        }),
        ("/sessions", []) => Ok(ParsedCommand::Sessions),
        ("/sessions", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/sessions",
        }),
        ("/resume", []) => Ok(ParsedCommand::Resume { id: None }),
        ("/resume", [id]) => Ok(ParsedCommand::Resume { id: Some(id) }),
        ("/resume", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/resume [id]",
        }),
        ("/accounts", []) => Ok(ParsedCommand::Accounts { id: None }),
        ("/accounts", [id]) => Ok(ParsedCommand::Accounts { id: Some(id) }),
        ("/accounts", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/accounts [id]",
        }),
        ("/login", []) => Ok(ParsedCommand::Login),
        ("/login", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/login",
        }),
        _ => Err(ParseError::UnknownCommand(command)),
    })
}

/// Returns registry entries whose names start with the command token being typed.
pub(crate) fn completions(input: &str) -> Vec<&'static SlashCommand> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(trimmed))
        .collect()
}

/// Returns complete command lines matching the MCP argument currently being typed.
pub(crate) fn mcp_completions(input: &str, servers: &[String]) -> Vec<String> {
    const ACTIONS: &[&str] = &["status", "start", "stop", "restart", "refresh"];
    let trimmed = input.trim_start();
    let Some(rest) = trimmed.strip_prefix("/mcp ") else {
        return Vec::new();
    };
    if rest.contains(char::is_whitespace) {
        let mut words = rest.split_whitespace();
        let Some(action) = words.next() else {
            return Vec::new();
        };
        let fragment = rest.strip_prefix(action).unwrap_or_default().trim_start();
        if !matches!(action, "start" | "stop" | "restart" | "refresh")
            || fragment.contains(char::is_whitespace)
        {
            return Vec::new();
        }
        return servers
            .iter()
            .filter(|server| server.starts_with(fragment))
            .map(|server| format!("/mcp {action} {server}"))
            .collect();
    }
    ACTIONS
        .iter()
        .filter(|action| action.starts_with(rest))
        .map(|action| format!("/mcp {action}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_unique_standard_commands() {
        assert_eq!(
            COMMANDS.iter().map(|c| c.name).collect::<Vec<_>>(),
            [
                "/model",
                "/statusbar",
                "/skills",
                "/tools",
                "/mcp",
                "/rewind",
                "/rules",
                "/new",
                "/sessions",
                "/resume",
                "/accounts",
                "/login",
                "/help",
                "/quit"
            ]
        );
        assert!(
            COMMANDS
                .iter()
                .all(|c| !c.description.is_empty() && c.usage.starts_with(c.name))
        );
    }

    #[test]
    fn parses_supported_forms() {
        assert_eq!(parse("/help"), Some(Ok(ParsedCommand::Help)));
        assert_eq!(parse("/statusbar"), Some(Ok(ParsedCommand::StatusBar)));
        assert_eq!(parse("/skills"), Some(Ok(ParsedCommand::Skills)));
        assert_eq!(parse("/tools"), Some(Ok(ParsedCommand::Tools)));
        assert_eq!(
            parse(" /model "),
            Some(Ok(ParsedCommand::Model {
                model: None,
                reasoning: None
            }))
        );
        assert_eq!(
            parse("/model gpt-5"),
            Some(Ok(ParsedCommand::Model {
                model: Some("gpt-5"),
                reasoning: None
            }))
        );
        assert_eq!(
            parse("/model gpt-5 high"),
            Some(Ok(ParsedCommand::Model {
                model: Some("gpt-5"),
                reasoning: Some("high")
            }))
        );
        assert_eq!(parse("ordinary prompt"), None);
    }

    #[test]
    fn rejects_unknown_commands_and_extra_arguments() {
        assert_eq!(
            parse("/nope"),
            Some(Err(ParseError::UnknownCommand("/nope")))
        );
        assert!(matches!(
            parse("/help now"),
            Some(Err(ParseError::InvalidUsage { .. }))
        ));
        assert!(matches!(
            parse("/model a b c"),
            Some(Err(ParseError::InvalidUsage { .. }))
        ));
    }

    #[test]
    fn parses_rewind_forms() {
        assert_eq!(
            parse("/rewind"),
            Some(Ok(ParsedCommand::Rewind {
                target: None,
                fork: false
            }))
        );
        assert_eq!(
            parse("/rewind 2"),
            Some(Ok(ParsedCommand::Rewind {
                target: Some(2),
                fork: false
            }))
        );
        assert_eq!(
            parse("/rewind 1 fork"),
            Some(Ok(ParsedCommand::Rewind {
                target: Some(1),
                fork: true
            }))
        );
        assert!(matches!(
            parse("/rewind fork"),
            Some(Err(ParseError::InvalidUsage { .. }))
        ));
    }

    #[test]
    fn parses_session_and_account_verbs() {
        assert_eq!(parse("/new"), Some(Ok(ParsedCommand::New)));
        assert_eq!(parse("/sessions"), Some(Ok(ParsedCommand::Sessions)));
        assert_eq!(
            parse("/resume"),
            Some(Ok(ParsedCommand::Resume { id: None }))
        );
        assert_eq!(
            parse("/resume abc123"),
            Some(Ok(ParsedCommand::Resume { id: Some("abc123") }))
        );
        assert_eq!(
            parse("/accounts"),
            Some(Ok(ParsedCommand::Accounts { id: None }))
        );
        assert_eq!(
            parse("/accounts chatgpt-2"),
            Some(Ok(ParsedCommand::Accounts {
                id: Some("chatgpt-2")
            }))
        );
        assert_eq!(parse("/login"), Some(Ok(ParsedCommand::Login)));
        assert!(matches!(
            parse("/new now"),
            Some(Err(ParseError::InvalidUsage { .. }))
        ));
        assert!(matches!(
            parse("/login extra"),
            Some(Err(ParseError::InvalidUsage { .. }))
        ));
    }

    #[test]
    fn filters_completions_by_prefix_only() {
        assert_eq!(completions("/").len(), COMMANDS.len());
        assert_eq!(
            completions("/m").iter().map(|c| c.name).collect::<Vec<_>>(),
            ["/model", "/mcp"]
        );
        assert!(completions("/model ").is_empty());
        assert!(completions("hello").is_empty());
    }

    #[test]
    fn completes_mcp_actions_and_servers() {
        let servers = vec!["filesystem".to_owned(), "github".to_owned()];
        assert_eq!(
            mcp_completions("/mcp st", &servers),
            ["/mcp status", "/mcp start", "/mcp stop"]
        );
        assert_eq!(
            mcp_completions("/mcp start f", &servers),
            ["/mcp start filesystem"]
        );
        assert_eq!(mcp_completions("/mcp restart ", &servers).len(), 2);
        assert!(mcp_completions("/mcp status ", &servers).is_empty());
    }
}
