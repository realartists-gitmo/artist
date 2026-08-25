# Rig 0.42 provider matrix

Phase 5 records Rig's provider surface as an input to `artist-provider`, not as
an application registration list. Every row is a descriptor installed by a
native driver; providers outside Rig use the same registry through the WASM
socket.

| Rig module | Completion/stream | Tools | Multimodal or extra APIs | Authentication / notes |
|---|---|---:|---|---|
| anthropic | yes | yes | images, reasoning, usage | API key |
| azure | yes | yes | OpenAI-compatible | API key / Azure endpoint |
| chatgpt | yes | yes | subscription-oriented | ChatGPT device/session auth |
| cohere | yes | yes | embeddings, rerank | API key |
| copilot | yes | yes | subscription-oriented | GitHub/Copilot OAuth |
| deepseek | yes | yes | reasoning | API key |
| doubleword | yes | yes | embeddings | API key |
| gemini | yes | yes | images, audio/transcription, embeddings, model listing | API key |
| groq | yes | yes | OpenAI-compatible, transcription | API key |
| huggingface | yes | yes | images, transcription | token |
| hyperbolic | yes | yes | OpenAI-compatible | API key |
| llamafile | yes | yes | local OpenAI-compatible | local endpoint / optional key |
| minimax | yes | yes | reasoning | API key |
| mira | yes | yes | provider-specific completion | API key |
| mistral | yes | yes | embeddings, transcription, model listing | API key |
| moonshot | yes | yes | OpenAI-compatible | API key |
| ollama | yes | yes | local models | local endpoint / optional key |
| openai | yes | yes | images, audio, embeddings, transcription, model listing | API key, project/org metadata |
| openrouter | yes | yes | images, embeddings, model listing | API key |
| perplexity | yes | yes | search-oriented completion | API key |
| together | yes | yes | embeddings | API key |
| venice | yes | yes | images, embeddings, transcription | API key |
| voyageai | embeddings/rerank | n/a | embeddings, rerank | API key (not completion) |
| xai | yes | yes | images, audio, reasoning | API key |
| xiaomimimo | yes | yes | provider-specific completion | API key |
| zai | yes | yes | provider-specific completion | API key |

Rig's `CompletionModel`/`StreamingCompletionResponse` normalize text, tool
calls and reasoning deltas, terminal usage/finish metadata, response IDs and
provider request IDs. Provider-native terminal records remain available in
Rig's raw channel. Capability discovery is therefore descriptor data, while
provider-specific terminal extensions stay in provider-private state or raw
metadata rather than kernel variants.

## What Rig does not own

Rig constructs clients and sends requests; it does not select durable Artist
accounts, persist credential references, refresh or revoke credentials, choose
an account default, maintain provider fallback epochs, discover installed
plugins, or provide a durable conversation chain. ChatGPT subscription and
Copilot OAuth are client authentication mechanisms, not account infrastructure.
`artist-provider` owns those concerns: credentials are resolved only at
`ProviderRegistry::resolve`, account selection is snapshotted in
`ResolvedProvider`, and opaque state is keyed by provider revision, account,
session, and profile epoch.

## Contract/conformance requirements

A driver must preserve ordered stream delivery, backpressure, cancellation,
terminal errors, retry metadata, tool-call correlation, usage and request IDs.
Native Rig drivers use `NativeRigDriver` and can return the existing
`artist-rig::RigModel`; arbitrary providers use `WasmDriver` and the explicit
request/event socket. A registry replacement affects new resolutions only;
an already resolved driver and its descriptor revision remain stable for the
attempt. State writes use optimistic CAS and file state commits by temporary
file plus atomic rename, so failed or cancelled attempts cannot silently
advance a provider chain.
