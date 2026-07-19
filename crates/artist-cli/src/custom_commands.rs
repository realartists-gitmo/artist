//! User-defined slash commands: markdown prompt templates discovered from
//! `~/.config/artist/commands/` and `<project>/.artist/commands/`.
//!
//! A command file is `<name>.md` with optional YAML frontmatter
//! (`description`); the body is the prompt template. `$ARGUMENTS` in the
//! body is replaced with everything typed after the command name (appended
//! if the marker is absent and arguments were given).

use std::path::Path;

use artist_rules::declarative::frontmatter;

const MAX_COMMANDS: usize = 100;
const TEMPLATE_CAP: u64 = 64 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct CustomCommand {
    /// Includes the leading slash.
    pub name: String,
    pub description: String,
    pub template: String,
}

/// Discover commands; project definitions shadow global ones by name, and
/// built-in command names always win (a custom `/help.md` is ignored).
pub(crate) fn discover(project: &Path) -> Vec<CustomCommand> {
    let mut roots = Vec::new();
    if let Some(config_root) = std::env::var_os("ARTIST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("artist")))
    {
        roots.push(config_root.join("commands"));
    }
    roots.push(project.join(".artist/commands"));
    let mut commands: Vec<CustomCommand> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .collect();
        files.sort();
        for file in files {
            let Some(command) = parse(&file) else {
                continue;
            };
            if crate::slash_commands::COMMANDS
                .iter()
                .any(|builtin| builtin.name == command.name)
            {
                continue;
            }
            if let Some(existing) = commands.iter_mut().find(|entry| entry.name == command.name) {
                *existing = command;
            } else if commands.len() < MAX_COMMANDS {
                commands.push(command);
            }
        }
    }
    commands
}

fn parse(file: &Path) -> Option<CustomCommand> {
    let stem = file.file_stem()?.to_str()?;
    let valid = !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return None;
    }
    if std::fs::metadata(file).ok()?.len() > TEMPLATE_CAP {
        return None;
    }
    let text = std::fs::read_to_string(file).ok()?;
    let (description, template) = match frontmatter(&text) {
        Ok((yaml, body)) => {
            #[derive(serde::Deserialize)]
            struct Frontmatter {
                description: Option<String>,
            }
            let parsed: Frontmatter = serde_yaml::from_str(yaml).ok()?;
            (
                parsed
                    .description
                    .unwrap_or_else(|| "custom command".to_owned()),
                body.to_owned(),
            )
        }
        // Frontmatter is optional for commands: the whole file is the template.
        Err(_) => ("custom command".to_owned(), text),
    };
    let template = template.trim().to_owned();
    if template.is_empty() {
        return None;
    }
    Some(CustomCommand {
        name: format!("/{stem}"),
        description,
        template,
    })
}

/// Completion candidates for a partially typed command token.
pub(crate) fn completions<'a>(
    commands: &'a [CustomCommand],
    input: &str,
) -> Vec<&'a CustomCommand> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('/') || trimmed.contains(char::is_whitespace) {
        return Vec::new();
    }
    commands
        .iter()
        .filter(|command| command.name.starts_with(trimmed))
        .collect()
}

/// If `content` invokes a custom command, expand its template. Returns the
/// expanded prompt content, or None for non-custom input. Expansion happens
/// once — an expanded template is never re-expanded.
pub(crate) fn expand_invocation(commands: &[CustomCommand], content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    let (token, arguments) = match trimmed.split_once(char::is_whitespace) {
        Some((token, rest)) => (token, rest.trim()),
        None => (trimmed, ""),
    };
    let command = commands.iter().find(|command| command.name == token)?;
    Some(expand(&command.template, arguments))
}

fn expand(template: &str, arguments: &str) -> String {
    if template.contains("$ARGUMENTS") {
        template.replace("$ARGUMENTS", arguments)
    } else if arguments.is_empty() {
        template.to_owned()
    } else {
        format!("{template}\n\n{arguments}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, template: &str) -> CustomCommand {
        CustomCommand {
            name: name.to_owned(),
            description: "d".to_owned(),
            template: template.to_owned(),
        }
    }

    #[test]
    fn expands_arguments_marker_or_appends() {
        let commands = vec![
            command("/review", "Review this PR: $ARGUMENTS. Be thorough."),
            command("/standup", "Summarize recent work."),
        ];
        assert_eq!(
            expand_invocation(&commands, "/review #42").as_deref(),
            Some("Review this PR: #42. Be thorough.")
        );
        assert_eq!(
            expand_invocation(&commands, "/standup").as_deref(),
            Some("Summarize recent work.")
        );
        assert_eq!(
            expand_invocation(&commands, "/standup for monday").as_deref(),
            Some("Summarize recent work.\n\nfor monday")
        );
        assert_eq!(expand_invocation(&commands, "/model"), None);
        assert_eq!(expand_invocation(&commands, "plain prompt"), None);
    }

    #[test]
    fn discovers_project_files_and_skips_builtin_shadows() {
        let dir = tempfile::tempdir().unwrap();
        let commands_dir = dir.path().join(".artist/commands");
        std::fs::create_dir_all(&commands_dir).unwrap();
        std::fs::write(
            commands_dir.join("review.md"),
            "---\ndescription: Review a PR\n---\nReview: $ARGUMENTS\n",
        )
        .unwrap();
        std::fs::write(commands_dir.join("bare.md"), "No frontmatter here.\n").unwrap();
        std::fs::write(commands_dir.join("help.md"), "shadow attempt\n").unwrap();
        let commands = discover(dir.path());
        let names: Vec<_> = commands
            .iter()
            .map(|command| command.name.as_str())
            .collect();
        assert!(names.contains(&"/review"));
        assert!(names.contains(&"/bare"));
        assert!(!names.contains(&"/help"), "builtins must win");
        let review = commands.iter().find(|c| c.name == "/review").unwrap();
        assert_eq!(review.description, "Review a PR");
    }
}
