# Security

Muse treats package files and lexical input as untrusted. Consumers should impose file-size,
collection-size, and recursion limits before loading externally supplied documents. Package
identity and envelope digests detect accidental or malicious mutation but do not authenticate the
publisher; signed distribution metadata should be added by the shipping environment.

The Rust workspace forbids unsafe code. No package may execute code during loading or validation.
