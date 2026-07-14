# llm-provider

Provider records and authentication primitives for Artist.

## Credential shapes

`SavedProvider` is deliberately serializable and supports:

- `Auth::ApiKey` for OpenAI-compatible `Bearer` API keys and custom base URLs.
- `Auth::ChatGpt` for access/refresh tokens and the ChatGPT workspace ID used by
  Codex subscription requests.

Secret values redact their `Debug` output, but serialization contains the real
value. Store serialized records in an OS keychain or encrypted credential store.

## ChatGPT login

`ChatGptOAuth::begin_login` creates an Authorization Code + PKCE URL and opaque
pending state. The UI should:

1. Bind a loopback listener and pass its callback URL to `begin_login`.
2. Open `LoginRequest::authorize_url` in the user's browser.
3. Read `code` and `state` from the callback.
4. Pass those values and `PendingLogin` to `finish_login`.
5. Save the returned `Auth::ChatGpt` in a `SavedProvider` whose base URL is
   `CHATGPT_CODEX_BASE_URL`.

Call `refresh` before `expires_at`; refresh-token rotation is handled.

The Codex OAuth client ID and OpenAI endpoints are service implementation
details and may change. The client ID can be overridden with `ChatGptOAuth::new`.
Using the official Codex public client ID from another application may be
restricted by OpenAI; production distribution should confirm OpenAI's terms and
register/use an approved client where available.
