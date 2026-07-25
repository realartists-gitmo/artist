# Subagent roles

Artist loads `$ARTIST_CONFIG_DIR/subagents.toml` (normally `~/.config/artist/subagents.toml`) and then `.artist/subagents.toml` in the project. Project roles replace global roles with the same name. The built-in `default`, `worker`, and read-only `explorer` roles remain available unless replaced.

```toml
[settings]
max_concurrent = 4

[agents.reviewer]
description = "Reviews a change for correctness"
model = "gpt-5"
reasoning_effort = "high"
instructions = "Review only; report findings with file and line references."

[agents.reviewer.tools]
allow = ["read", "find", "grep"]
deny = ["bash"]
```

`description` is required. Model and reasoning settings inherit from the parent provider when omitted. `allow` intersects the parent's available tools and `deny` is then removed; deny always wins. Supported policy names are `bash`, `read`, `find`, `grep`, `edit`, `write`, `skill`, and `subagent`. Subagents cannot invoke `subagent`, even if it appears in `allow`; listing it in `deny` is accepted for clarity. Invalid tool names produce a configuration diagnostic and disable that role definition.

The model selects a role with the `agent` field of the `subagent` tool. Lifecycle operations (`run`, `start`, `status`, `read`, `wait`, `cancel`, and `list`) and `fork` remain available. Instance/task IDs are generated independently of role names, so a role can run more than once concurrently.
