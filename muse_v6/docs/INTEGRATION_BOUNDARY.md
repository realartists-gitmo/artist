# Integration boundary

The semantic foundation and cognitive superstrate now live in one Muse Cargo workspace. This removes the packaging boundary only; it does not fabricate the still-missing semantic-to-formal lowering bridge.

# Future superstrate integration boundary

Muse and the cognitive superstrate remain independent workspaces.

```text
Muse InterpretationBundle
        |
        v
future muse-superstrate-adapter
        |
        v
exact superstrate InterpretedGraph + ontology declarations
```

The adapter will bind:

- Muse semantic object IDs to formal object IDs.
- Muse concept IDs to formal ontology type IDs.
- Muse relation IDs to formal ontology symbols.
- Muse package-snapshot identity to the formal submission certificate.

Artist-specific policy belongs in an `artist-muse` adapter outside both workspaces. Mnestic stores
pinned artifacts and evidence but does not determine their meaning at read time.
