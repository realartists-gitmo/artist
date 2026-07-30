# Artist
A new opinionated, highly customizable, performant, and minimal coding harness.
<img width="1920" height="389" alt="image" src="https://github.com/user-attachments/assets/f44553c4-70d3-46b1-80a3-fce8a030d3e5" />


## Stability Warning

Artist is **early in development**. Updates can and probably will **break extensions**. Expect bugs.

## Runtime transition

The previous Rig-native provider and chat runtime has been removed. The `artist`
binary currently exposes only offline rule and session maintenance commands and
cannot authenticate with, discover, or call model APIs. Existing provider
credential files are left untouched for the upcoming project-owned runtime.

## Features Roadmap
- [ ] Project-owned model runtime
- [ ] MCP
- [ ] Skills
- [ ] Extensions, with customizable tools
- [ ] AGENTS.md, and .agents/skills protocol support
- [ ] First class Herdr support
- [ ] ACP support
- [ ] Customizable system prompt
- [ ] Customizable TUI
- [ ] GPUI-based desktop app.

## Contributing
Bug fix PRs are welcome. For new features, create an issue discussion first.
