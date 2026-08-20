# Design

The kernel needs to be aggressively minimal---only what's necessary for the fundamentals of the harness. That means:
Fuser
Windows fsys compatl ayer
(For the VFS)
Wasmtime

toon-rust (NO JSON)

Namespaces---we want to exclude implementing most of these from the core kernel and make them modular, including even very basic stuff like tools://. The one exception is resources://, which is the namespace that handles namespace extensions: it lives within the kernel. Every other namespace is defined inside resources://.

Security, sandboxing, etc. are non-issues. All code should be given full power, trusted, and run as-is.

Important thing about 'everything is a file.' Our goal is for our harness to durably address WITHIN files via anchors, not via line numbers or exact string matching. The 'teca' crate is for this exact purpose. In an ideal world, I would PREFER that FUSE handles this at a very high level, since FUSE is also how we're gonna plug stuff into bash, and I don't want to maintain an individual bash compatibility layer for each tool that can read lines and get line numbers where they're changed out for anchors. But I'm not sure if this ask is possible, we need to consider it. Maybe we can vendor uutils or something.
^ Revised 2026-08-18: addressing is no longer kernel-resident. It is a tool-surface convention of the read/edit verbs; the teca engine lives in the tool layer
