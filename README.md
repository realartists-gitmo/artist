# Artist
A new opinionated, highly customizable, performant, and minimal coding harness.
<img width="1920" height="389" alt="image" src="https://github.com/user-attachments/assets/f44553c4-70d3-46b1-80a3-fce8a030d3e5" />


## Stability Warning

Artist is **early in development**. Updates can and probably will **break extensions**. Expect bugs.

## Why use it?
Of course there are many coding harnesses out there. We built Artist because every coding harness had a set of problems to reconcile:
- **Claude Code** (and Codex, but much less): massive frontend specific instructions and form guidance inapplicable to most devs. Artist mirror's **Pi**'s minimal prompt with a customizable system, so your models act how **you** want and preserve context.
- **Pi** and **OpenCode**: Typescript-managed harnesses and TUIs makes parallel usage explode system resource usage. Artist is built on **Rig** and **Ratatui** in **Rust**, creating a small native executable. Artist extensions are compiled to WASM, meaning they can be developed in any supported language but will execute fast while maintaining customizability.

Later, a Codex-app-like desktop app developed in native-Rust via **GPUI** will be implemented, creating a performant desktop app experience.

## Model Compatibility Roadmap
- [x] Rig provided models
- [ ] Generic OpenAI key support
- [x] ChatGPT subscription support
- [ ] ZAI, OpenCode, other subscription support

## Features Roadmap
- [x] MCP
- [x] Skills
- [x] Extensions, with customizable tools
- [x] AGENTS.md, and .agents/skills protocol support
- [ ] First class Herdr support
- [ ] ACP support
- [ ] Customizable system prompt
- [ ] Customizable TUI
- [ ] GPUI-based desktop app.

## Contributing
Bug fix PRs are welcome. For new features, create an issue discussion first.

### Tool schema guidance
Tool argument instructions belong in the JSON Schema property where the argument is used. Add `description` to individual properties (including nested object and array-item properties) for argument-specific constraints, examples, and formatting rules. Keep the top-level tool description focused on the operation and its broad lifecycle; do not duplicate property-level guidance there unless provider compatibility requires it.
