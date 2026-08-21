https://rig.rs/ -- MANDATORY
https://github.com/ForeverAngry/rig-tap

You are not permitted to inspect the git history of this project.

Minimize lines of code, write clean, elegant, non-leaky and non-overengineered abstractions. Do not overengineer or overslop.

Explicitly deferred:
Frontend, UI.

Anti-features (MUST NOT BE INCLUDED AT ALL):
- Permissions
- Sandboxing
- Security
- Isolation, whether via git worktrees or whatever

General purpose agentic harness with default plugins optimized for coding, where 'everything is a plugin.'

HORIZONTAL DEVELOPMENT, NOT VERTICAL. THAT MEANS WE DO NOT NEED PROOF, OR END TO END FEATURES. WE NEED TO BUILD ELEGANTLY.

Plan:
Must be streaming agent only
1. Rig core, harness kernel. Needs to abstract over communicating with LLMs. Important things:
  - A session is a persistent, accumulating conversation with a particular agent. Rig handles this for us automatically, though we ofc do have to serialize it. https://rig.rs/docs/concepts/memory. (We do not need to handle single-shots, we are a persistent product)
  - At the top of each session, immediately before the user's first prompt, is an arbitrary combination of context. This could be SYSTEM.md + AGENTS.md, or whatever other bullshit configuration--we need to be ready for all of it. I believe Rig will handle the tool side of this for us, but im unsure.
  - Things that can happen in the conversation:
  - Input gets sent to the model. This will be used for normal user prompts, hook updates, etc.
  - Input gets 'steered', instead of being directly inserted into the convo with the model. This is used for notifications or attaching information while the model is working. You should know how steering works.
  - Stream gets aborted and model is stopped where it is.
  - ^ all of the above can be done either by the user or the harness automatically, so we need to have an elegant, ergonomic, and consistent API. Tell me if im missing something pls.
  - By construction, we should NEVER invalidate the cache prefix---ergo, with the sole exception of compaction policies at context limits, we should never alter session history. Include https://docs.rs/rig-memory/latest/rig_memory/---the default compaction policies are fine to include as a set of options, we also will have a WASM extensibility socket that allows us to build our own context handling policies when limits are approached to memory.
2. Core extensibility runtime, WASMtime, need a unified and elegant WIT component contract.
3. Extensibility sockets though they are not exercised yet:
a. prompt composition between SYSTEM.md, AGENTS.md, and whatever else vis a vis context
b. tools
c. context (wired into rig-memory, specced above)
d. hooks https://rig.rs/docs/concepts/hooks
e. tell me if im missing something obvious.

## URI resource fabric

The canonical tool substrate is one UTF-8 resource tree. `ResourceUri` uses a
normal URL path for the base resource and the complete query as a slash-delimited
projection path. A node may have both text and logical children. Providers
register operation-scoped base/projection globs; routing is deterministic by
literal specificity, wildcard count, and load order.

The only model-facing verbs are `read`, `find`, `grep`, `write`, `move`, and
`poll`. The same `ToolRegistry` is used by Rig dynamic tools and WASM host
callbacks. Calls carry one correlation ID and a stack, so direct and indirect
plugin recursion fails before component re-entry.

On Linux, `ResourceFabric` owns an ephemeral FUSE projection and one FFF 0.10.5
index over the complete mount. URI queries appear as separate Unix names such as
`rust.rs?symbols/`; shells start in the projected `file` subtree and receive the
mount root as `ARTIST_ROOT`. FUSE attributes exist only to satisfy the kernel.

The component ABI is `artist:plugin@0.3.0`. Tool providers and resource
providers are separate exports; prompt, context, hooks, model configuration,
and event lifecycle exports remain intact. The default component supplies the
terminal filesystem route, while the host owns native filesystem mechanics,
FUSE, FFF, and the universal verbs.
