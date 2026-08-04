# Artist over OpenAI Secure MCP Tunnel

Artist exposes Streamable HTTP only on loopback at `http://127.0.0.1:8317/mcp`. Do not bind it publicly: the surface can include shell, file mutation, computer control, canvas, delegation, and communications.

## Install

Build and install the binary, then install the repository-owned wrappers and user units:

```bash
cargo build --release -p artist-mcp-server
install -m 0755 target/release/artist-mcp ~/.local/bin/artist-mcp
scripts/install-mcp-services
```

Create `~/.config/artist/mcp-tunnel.env` with mode `0600`:

```bash
CONTROL_PLANE_TUNNEL_ID=...
CONTROL_PLANE_API_KEY=...
```

The repository never stores these values. Optional daemon settings belong in `~/.config/artist/mcp-daemon.env`; optional tunnel process settings belong in `~/.config/artist/mcp-tunnel-service.env`.

Enable both services:

```bash
systemctl --user enable --now artist-mcp.service artist-mcp-tunnel.service
```

## Workspace selection

The `workspace` MCP tool persists the directory Artist should open on its next daemon start. A selection never changes the root of a live process. After `workspace {"action":"select","path":"/path/to/project"}`, restart `artist-mcp.service`. With no selection, the service uses `ARTIST_MCP_DEFAULT_PROJECT`.

## Recovery

Calls carrying `_meta.idempotencyKey` are fsynced to an append-only operation ledger. Retry the same logical call with the same key to replay its exact result, or use the read-only `operation` tool to list recent keyed calls and recover one by key.

## Migration from RealArtist

The old `realartist.service` wrapped tunnel startup in another repository. After the new tunnel environment file exists and `artist-mcp-tunnel.service` is healthy:

```bash
systemctl --user disable --now realartist.service
systemctl --user enable --now artist-mcp-tunnel.service
```

Keep the old unit disabled rather than running two tunnel clients against the same tunnel identity.
