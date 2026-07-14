# llm-provider

ChatGPT subscription authentication and provider records for Artist.

`SavedProvider` stores OAuth access/refresh tokens, ChatGPT workspace identity,
and the selected Codex model and reasoning effort. Secrets are redacted from
`Debug`, but serialization contains their real values; persist provider records
only in a protected credential file or OS keychain.

## Login

`ChatGptOAuth::begin_login` starts Authorization Code + PKCE. The caller binds a
loopback callback server, opens the returned authorization URL, and passes the
callback code and state to `finish_login`. Call `refresh` before token expiry and
persist the returned rotated credentials immediately.

The Codex OAuth client ID and OpenAI endpoints are service implementation details
and may change. Production distribution should confirm OpenAI's terms and use an
approved OAuth client where available.
