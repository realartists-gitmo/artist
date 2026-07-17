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
        name: "/mcp",
        description: "Manage MCP servers",
        usage: "/mcp [status|start|stop|restart|refresh] [server]",
    },
    SlashCommand {
        name: "/help",
        description: "Show available commands",
        usage: "/help",
    },
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ParsedCommand<'a> {
    Help,
    Skills,
    StatusBar,
    Mcp {
        action: &'a str,
        server: Option<&'a str>,
    },
    Model {
        model: Option<&'a str>,
        reasoning: Option<&'a str>,
    },
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
        ("/statusbar", []) => Ok(ParsedCommand::StatusBar),
        ("/statusbar", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/statusbar",
        }),
        ("/skills", _) => Err(ParseError::InvalidUsage {
            command,
            usage: "/skills",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_unique_standard_commands() {
        assert_eq!(
            COMMANDS.iter().map(|c| c.name).collect::<Vec<_>>(),
            ["/model", "/statusbar", "/skills", "/mcp", "/help"]
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
    fn filters_completions_by_prefix_only() {
        assert_eq!(completions("/").len(), 5);
        assert_eq!(
            completions("/m").iter().map(|c| c.name).collect::<Vec<_>>(),
            ["/model", "/mcp"]
        );
        assert!(completions("/model ").is_empty());
        assert!(completions("hello").is_empty());
    }
}
