# Subagent roles

Artist loads `$ARTIST_CONFIG_DIR/subagents.toml` (normally `~/.config/artist/subagents.toml`) and then `.artist/subagents.toml` in the project. Project roles replace global roles with the same name. The built-in `default`, `worker`, and read-only `explorer`, `planner`, and `reviewer` roles remain available unless replaced.

```toml
[settings]
max_concurrent = 4

[agents.reviewer]
description = "Reviews a change for correctness"
model = "gpt-5"
reasoning_effort = "high"
instructions_file = "prompts/reviewer.md"

[agents.reviewer.tools]
allow = ["read", "find", "grep"]
deny = ["bash"]
```

`description` is required. `instructions_file` is resolved relative to the `subagents.toml` that declares it, so project configuration naturally refers to project-local files. It is mutually exclusive with inline `instructions`; a definition containing both is rejected with a diagnostic. Missing, unreadable, or empty prompt files also reject only that definition, leaving an earlier or built-in role safely available. Model and reasoning settings inherit from the parent provider when omitted. `allow` intersects the parent's available tools and `deny` is then removed; deny always wins. Supported policy names are `bash`, `read`, `find`, `grep`, `edit`, `write`, `skill`, and `subagent`. Subagents cannot invoke `subagent`, even if it appears in `allow`; listing it in `deny` is accepted for clarity. Invalid tool names produce a configuration diagnostic and disable that role definition.

On startup Artist creates editable defaults under `$ARTIST_CONFIG_DIR/prompts` and a default `subagents.toml` only when each file is absent; existing files are never overwritten. The main agent uses `prompts/main.md`, falling back to its embedded prompt with a diagnostic when the custom file cannot be read or is empty.

The model selects a role with the `agent` field of the `subagent` tool. Lifecycle operations (`run`, `start`, `status`, `read`, `wait`, `cancel`, and `list`) and `fork` remain available. Instance/task IDs are generated independently of role names, so a role can run more than once concurrently.
