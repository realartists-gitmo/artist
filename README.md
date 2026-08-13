# Artist

Artist is a Rust coding-agent harness being rebuilt around an
“everything is a file” virtual filesystem.

The repository is currently in the destructive first phase of that refactor:
the old bash, built-in file tools, hashline coordinator, MCP, extensions,
stream rules, subagents, and compaction surfaces have been removed. The
remaining binary can authenticate with providers, create/resume sessions, and
run the minimal model loop while the VFS kernel is built.

The intended model-facing contract is one universal set of verbs:

```text
read write edit send poll delete find grep
```

Schemes such as `repo://`, `agent://`, `session://`, `bash://`,
`mcp://`, and `tools://` will identify address roots. Node kinds—not schemes—
define serialization and capabilities. See [gort(1).md](gort(1).md) for the
design decisions and [docs/architecture.md](docs/architecture.md) for the
current implementation boundary.
