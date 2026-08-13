# llm-provider

Provider records, authentication, and the completion-provider registry used by
Artist. Artist supports these **23 completion providers**:

| Provider kind | Provider kind | Provider kind |
|---|---|---|
| `anthropic` | `azure` | `chatgpt` |
| `cohere` | `copilot` | `deepseek` |
| `gemini` | `groq` | `huggingface` |
| `hyperbolic` | `llamafile` | `minimax` |
| `mira` | `mistral` | `moonshot` |
| `ollama` | `openai` | `openrouter` |
| `perplexity` | `together` | `xai` |
| `xiaomimimo` | `zai` | |

Voyage AI is not listed because Rig's Voyage integration is embedding-only;
Artist requires a completion model.

## Configure providers

Use first-run setup to configure credentials and the default provider. Inside
the TUI, `/login` starts or refreshes ChatGPT subscription login, while
`/model` selects the model and reasoning effort. The standalone CLI equivalent
for model selection is `artist model`. ChatGPT subscription login uses PKCE;
OpenAI API-key mode is a separate login choice. Provider/account CRUD commands
from older releases are no longer part of the public CLI.

Provider records live in `$ARTIST_CONFIG_DIR/providers.toml` (normally
`~/.config/artist/providers.toml`). The v4 shape is:

```toml
version = 4
default_provider = "local"

[[providers]]
id = "local"
name = "Local Ollama"
provider = "ollama"
base_url = "http://localhost:11434/"
model = "qwen3-coder"                 # belongs to this provider
[providers.credentials]
type = "none"

[[providers]]
id = "work-azure"
name = "Work Azure"
provider = "azure"
base_url = "https://YOUR-RESOURCE.openai.azure.com/"
api_version = "2024-10-21"
model = "YOUR-DEPLOYMENT"
[providers.credentials]
type = "bearer_token"
token = "REPLACE_ME"
```

Examples deliberately contain placeholders, not usable secrets. Credential
forms are explicitly tagged: `none`, `api_key` (`api_key = "..."`),
`bearer_token` (`token = "..."`), `copilot_oauth` (`token_dir = "..."`),
and `chatgpt` (OAuth token/account fields). `api = "responses"` or
`api = "chat_completions"` selects an OpenAI-compatible protocol where that
provider supports a choice. Azure additionally accepts API-key or bearer-token
auth and an `api_version`. Local Llamafile needs no direct credential; Ollama
accepts `none` or an API key. Other hosted providers normally use `api_key`.

ChatGPT is different from the OpenAI API: it uses Authorization Code + PKCE
against the Codex backend and requires an eligible **ChatGPT subscription**;
an OpenAI API key is not a substitute. The login stores access/refresh tokens,
account/workspace identity, and expiry. GitHub Copilot supports a Copilot API
key, a GitHub bearer token, or device OAuth (`copilot_oauth`); device flow may
open during interactive add/test and caches tokens under `token_dir`.

## Models and protocols

`model` and `reasoning_effort` are provider-local. Switching providers restores
that provider's choices instead of carrying an incompatible model across
services. `artist model` and `/model` update the active default provider.
OpenAI can use Responses (the default) or Chat Completions. MiniMax, Moonshot,
Xiaomi MiMo, and Z.ai expose their alternate Anthropic-compatible transport via
the protocol choice; most remaining providers use their native Rig client.
The test command is the quickest way to detect a wrong model, endpoint,
protocol, or credential combination.

## Migration and security

Loading a pre-v4 file migrates untagged legacy ChatGPT `auth` records to
`credentials.type = "chatgpt"`, preserves old API-key records, and writes v4
on the next save. The earlier `~/.artist` config root is migrated once to the
platform config directory without overwriting destination files.

The config directory is restricted to `0700` and `providers.toml` to `0600` on
Unix; Copilot token directories/files are similarly tightened. `Secret` values
are redacted from `Debug`, but serialization necessarily contains real tokens.
Do not commit, paste, or back up this file to an untrusted location. OAuth
refresh tokens are rotated and the returned credentials must be saved
immediately. JWT identity is decoded only to obtain account metadata; trust
comes from the OAuth token source, not local signature verification. OAuth
client IDs and service endpoints are external implementation dependencies and
may change.
