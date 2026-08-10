# JSON package format

Files ending in `.muse.json` contain a `DocumentEnvelope`:

```json
{
  "format_version": "muse-json-1",
  "document_digest": { "algorithm": "sha256", "value": "..." },
  "payload": {
    "document_kind": "package",
    "document": {
      "kind": "ontology",
      "package": { "header": {}, "concepts": {}, "relations": {}, "axioms": [] }
    }
  }
}
```

The document digest is SHA-256 over the compact canonical JSON serialization of `payload`.
Package identity has a separate source/content pin inside its header. `muse-io` verifies the
envelope before registering a package and writes files atomically through a temporary sibling.

## Package content digest

A package reference digest is SHA-256 over the canonical serialized `SemanticPackage` after replacing only its own `header.package.digest` with 64 zeroes. Exact import pins remain in the digest. This removes circularity while binding the package identity to all substantive content and dependency pins. Registry loading and document verification reject a mismatched declared package digest.
