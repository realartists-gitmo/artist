# Open-ended architecture gate

This is the refactor-only gate for the final architecture. It is intentionally strict: it must remain failing until the legacy migration is complete, and then become a zero-match check.

Run from the repository root:

```bash
set -eu
! rg -n 'enum Verb|Verb::ALL|enum Operation|invoke_[a-z]+_typed|trait Handler|Kernel\.handlers' \
  crates/artist-kernel/src crates/artist-component/src crates/artist-cli/src
! rg -n 'match (verb|operation)|match request\.verb' \
  crates/artist-kernel/src crates/artist-component/src
cargo fmt --all -- --check
git diff --check
cargo test --workspace
```

Forbidden final-architecture symbols:

- Closed installed-verb enums (`Verb`, `Operation`, `Verb::ALL`).
- Per-verb invocation functions or match-arm dispatch.
- Legacy `Handler` and JSON resource ABI.
- Internal JSON dynamic invocation/coercion.
- Kernel-known model-tool registration tables.
- Fixed resource export vocabularies.

Allowed JSON boundary: external model/CLI adapters only, never routing, claims,
package activation, resource providers, or Component Model invocation.

Current status: **ready for the final verification pass**. JSON remains only in
the explicit model/CLI/component adapter modules and is not part of kernel
routing, claims, package activation, or component invocation.
