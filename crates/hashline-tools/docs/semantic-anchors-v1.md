# Semantic anchors v1

File anchors are deterministic addresses of complete occurrence identities. No allocator state is persisted or consulted.

Structured languages use `artist-ast` and its existing ast-grep/tree-sitter stack. The shared extractor serializes language, node kind, parent field/role, governing named-node ancestry with available semantic keys, canonical leaf content for the occurrence, and the equivalent-occurrence rank. Parserless text uses exact logical-line bytes plus the equivalent-exact-line rank.

The serialization is binary TLV under the `artist.anchor.identity.v1\0` domain. It does not contain file/path, line number, byte offset, neighbor identity, duplicate count, read history, or an address collision result.

The canonical identity bytes are passed directly to the frozen `anchor_address_v1` function. Field values map directly to row `u` of `anchor_tokens_68399.txt`. The token artifact contains a small number of duplicate payload rows, so live prefix selection compares rendered payloads; a duplicate payload extends the textual address just like a numeric component collision.

Text grammar is `#TOKEN` for one component and `#TOKEN‖TOKEN[‖TOKEN...]` for longer prefixes. U+2016 `‖` does not occur in the frozen token payloads. The shortest rendered prefix unique among live occurrences is shown. Resolution is exact opaque string equality; callers must not trim, case-fold, Unicode-normalize, or fuzzy-match anchors.
