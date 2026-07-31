# ChatGPT web integration through component-owned tools and Veilid

## Status

This document specifies the intended final architecture for connecting one
published Artist app in ChatGPT to user-owned Artist sessions and devices.

Artist does not adopt knowingly provisional product architectures. Work may be
split into independently testable changes, but each change must preserve and
converge on the design specified here.

The decisive architectural choice is:

> The shared remote MCP service exposes only a harmless bootstrap tool. After
> the Artist component is mounted, the component registers the real Artist tools
> directly with the ChatGPT host. Sensitive tool calls travel from the host to
> the component and then over Veilid to `artistd`; they never pass through the
> shared remote MCP service.

## Core invariants

The implementation is invalid if any of these invariants is violated:

1. ChatGPT is the sole agent. No local model, planner, nested agent loop, or
   autonomous tool selector participates in the web integration.
2. The shared remote MCP service never receives repository paths, commands,
   patches, source text, search terms, session names, tool results, or local
   credentials.
3. The only remotely registered model-facing tool is the bootstrap tool
   `artist_open`.
4. `read`, `find`, `grep`, `edit`, `write`, `bash`, session tools, and exported
   extension tools are registered by the mounted Artist component through the
   MCP Apps app-side tool interface.
5. The component is deterministic transport and protocol code. It executes only
   the exact tool selected by ChatGPT with the exact host-delivered arguments.
6. Every local request is signed by an authorized component key, scoped to an
   explicit Artist attachment, and encrypted before entering Veilid.
7. Only the destination `artistd` authorizes and dispatches local tools.
8. Every result is signed by the destination device and verified by the
   component before it is returned to ChatGPT.
9. Discovery, relay, bootstrap, and remote MCP edge operators possess no local
   computer authority and see no plaintext tool payloads.
10. The component executable is an immutable reviewed artifact; an edge cannot
    substitute executable code.
11. Artist has one canonical tool registry. Rig, app-local MCP tools, and Veilid
    RPC dispatch are adapters over that registry, not parallel tool systems.
12. Imported MCP tools and agent-internal controls are never recursively
    exported by default.
13. A transport connection is not a durable session identity. Every request
    names an explicit attachment or attachment capability.
14. Destructive requests are replay-safe and idempotent.
15. Long-running commands remain local and do not hold a remote HTTP request
    open.

## Goals

The system provides all of the following simultaneously:

- one published Artist app with one stable identity in ChatGPT;
- no per-user custom MCP endpoint setup;
- one-time pairing between a ChatGPT installation and a user-owned Artist
  account;
- reuse of that pairing across later conversations;
- multiple user devices and multiple Artist sessions;
- multiple ChatGPT conversations attached to one session concurrently;
- dynamic, individually typed Artist tools projected from the authorized daemon;
- end-to-end confidentiality and authorization for every sensitive tool call;
- direct peer connectivity where available and encrypted relay fallback where
  required;
- no central device registry, session registry, reverse-tunnel fleet,
  repository service, shell service, or per-user cloud runtime;
- replaceable community-operated bootstrap, discovery, relay, and remote MCP
  edges;
- explicit revocation and recovery for components, devices, sessions, and
  account identity.

## Non-goals

The ChatGPT integration does not run Artist's local LLM agent loop. ChatGPT owns
conversation history, planning, tool selection, and decisions about what to do
next.

The Artist component does not interpret goals, rewrite commands, choose tools,
run a model, or delegate work to subagents. It validates, signs, encrypts,
transports, verifies, and returns exact requests and results.

The remote MCP service does not execute Artist tools, route to devices, retain
repositories, proxy shell output, authenticate local commands, or maintain
per-user runtime state.

## System overview

```text
                           one published Artist app
                                      │
                                      │ ordinary remote MCP
                                      ▼
                     shared Artist bootstrap service
                  ┌────────────────────────────────────┐
                  │ initialize                         │
                  │ tools/list: artist_open only       │
                  │ tools/call: artist_open only       │
                  │ immutable component resource       │
                  │ no sensitive input or output       │
                  └──────────────────┬─────────────────┘
                                     │ mounts reviewed component
                                     ▼
                            ChatGPT host + component
                  ┌────────────────────────────────────┐
                  │ app.registerTool(read/edit/bash…) │
                  │ model-visible app-side tools       │
                  │ pairing and attachment state       │
                  │ request signing and encryption     │
                  │ Veilid browser/WASM node           │
                  └──────────────────┬─────────────────┘
                                     │ encrypted Veilid AppCall
                      ┌──────────────┴──────────────┐
                      │ Veilid DHT/private routes  │
                      │ direct path/relay fallback │
                      │ community-operated mesh    │
                      └──────────────┬──────────────┘
                                     │
                                     ▼
                              destination artistd
                  ┌────────────────────────────────────┐
                  │ verifies key/capability/nonce      │
                  │ resolves explicit attachment       │
                  │ dispatches canonical Artist tool   │
                  │ records event and signs result     │
                  └────────────────────────────────────┘
```

The architectural split is:

- **Remote MCP mounts Artist.**
- **The component exposes the real tools.**
- **Veilid transports encrypted calls.**
- **`artistd` authorizes and executes.**
- **ChatGPT remains the agent.**

## Shared remote MCP bootstrap service

The published app uses one stable endpoint, for example:

```text
https://mcp.artist.network/mcp
```

All conforming edges serving that endpoint implement the same minimal protocol.
The remote tool list contains exactly one model-visible tool:

```text
artist_open
```

`artist_open` accepts no sensitive arguments. At most it accepts presentation
preferences that reveal nothing about the user's devices or workspaces.

Representative schema:

```json
{
  "name": "artist_open",
  "description": "Open the paired Artist connection and make its authorized tools available.",
  "inputSchema": {
    "type": "object",
    "properties": {},
    "additionalProperties": false
  }
}
```

The service responsibilities are limited to:

- MCP initialization and extension negotiation;
- publishing the `artist_open` schema;
- returning the immutable Artist component resource;
- returning a non-sensitive bootstrap result;
- serving signed release and compatibility metadata.

It must not:

- publish `read`, `edit`, `bash`, or any other local tool remotely;
- receive or mirror app-local tool arguments;
- receive or mirror app-local tool results;
- know the paired Artist account, device, session, or attachment;
- hold a device connection;
- route local requests;
- issue a credential capable of authorizing local execution;
- serve edge-specific executable component code.

A representative successful call is:

```json
{
  "content": [
    {
      "type": "text",
      "text": "Artist is open. Its paired tools are supplied by the Artist component."
    }
  ],
  "structuredContent": {
    "state": "component_mounted"
  }
}
```

No local request is acknowledged through this tool. The old architecture in
which every remote tool call returned a delegation acknowledgement is removed.

## Immutable component publication

The component is security-critical because it holds or accesses the private key
that authorizes local requests. Community edges are not trusted to choose its
bytes.

The published Artist app binds the component to an immutable, content-addressed
release artifact. The binding contains at least:

```rust
struct ComponentRelease {
    release_id: ComponentReleaseId,
    artifact_digest: Digest,
    protocol_range: VersionRange,
    tool_registry_schema_version: u32,
    release_signature: ArtistReleaseSignature,
}
```

Required behavior:

- app publication records the accepted component artifact or digest;
- the ChatGPT host loads only the reviewed artifact matching that binding;
- edge responses may reference the artifact but cannot replace it;
- dynamic edge-specific HTML or JavaScript is forbidden;
- component updates use a new signed release identity and normal app review;
- a component refuses to use pairing credentials when its release identity is
  not authorized by the paired account policy.

Whether the host pins bytes directly, pins a digest, or resolves an immutable
publication object is an integration detail. The security property is mandatory.

## Artist component as an app-local MCP tool server

After `artist_open` mounts the component, the component connects to the MCP Apps
host bridge and advertises app-side tool capability.

It registers authorized Artist tools using the app-side tool API:

```typescript
const app = new App(
  { name: "Artist", version: COMPONENT_VERSION },
  { tools: { listChanged: true } },
);

app.registerTool(
  "read",
  {
    description: "Read a file from the attached Artist workspace",
    inputSchema: readSchema,
    _meta: { ui: { visibility: ["model", "app"] } },
    annotations: { readOnlyHint: true },
  },
  async (args) => invokeArtist("read", args),
);

await app.connect();
```

The host discovers these tools from the component's app-side `tools/list` and
calls them through the component's app-side `tools/call`. These invocations do
not traverse the remote MCP connection.

The component is responsible for:

- durable pairing state for the ChatGPT installation;
- obtaining a signed registry projection from an authorized daemon;
- registering, updating, disabling, and removing app-local tools;
- issuing tool-list-changed notifications when the authorized registry changes;
- maintaining the active conversation attachment;
- validating host-delivered tool arguments against the signed schema;
- constructing signed, replay-safe request envelopes;
- encrypting them to the destination device or route;
- sending and receiving through Veilid;
- verifying signed daemon results;
- returning ordinary MCP `CallToolResult` objects to the host;
- surfacing deterministic connection, permission, and failure states.

It must not:

- run a language model;
- select a tool;
- infer user intent;
- modify tool arguments except canonical serialization;
- fall back to a local Artist agent;
- execute filesystem or shell operations itself;
- accept an unsigned registry or result;
- return a daemon result whose request ID, attachment, device, or signature does
  not match the pending invocation.

## Tool visibility and lifecycle

While the component is mounted and paired, its authorized app-local tools are
model-visible.

When the component is absent, the only Artist tool visible through the remote
service is `artist_open`. The model can call it to remount the component.

Lifecycle:

```text
component absent
    → remote tool set contains artist_open

model calls artist_open
    → component mounts and connects
    → component restores pairing
    → component discovers authorized daemon/session state
    → component registers app-local tools

component active
    → model calls app-local read/edit/bash/etc.

component teardown
    → app-local tools disappear
    → artist_open remains available
```

Component teardown never causes a sensitive call to fall back through the
remote server. A call either reaches the registered component tool or fails with
a structured remount-required error.

The component should request a durable control surface, such as picture-in-
picture where the host supports it, but correctness cannot depend on indefinite
iframe lifetime.

## Canonical tool registry

Artist has one canonical tool descriptor and dispatch interface:

```rust
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub annotations: ToolAnnotations,
    pub origin: ToolOrigin,
    pub export_policy: ExportPolicy,
    pub schema_version: u32,
}

pub trait ArtistTool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    fn call<'a>(
        &'a self,
        context: &'a ToolCallContext,
        arguments: serde_json::Value,
    ) -> BoxFuture<'a, ArtistToolResult>;
}
```

Adapters consume the same registry:

```text
ArtistTool → local Rig ToolDyn
ArtistTool → signed app-local registry projection
ArtistTool → artistd Veilid RPC dispatch
```

The remote bootstrap service does not project this registry. Only a paired
component receives the projection from an authorized daemon.

Tool provenance is explicit:

```rust
enum ToolOrigin {
    Native,
    Skill,
    Extension { id: String },
    ImportedMcp { server: String },
    AgentInternal,
}

enum ExportPolicy {
    Always,
    Explicit,
    Never,
}
```

Default component export policy:

| Origin | Default |
|---|---|
| Native built-ins | Always |
| Trusted extensions | Explicit |
| Skills | Explicit |
| Imported MCP tools | Never |
| Subagent/internal controls | Never |

The daemon signs the exact registry projection:

```rust
struct SignedToolRegistry {
    account: AccountPublicKey,
    device: DeviceId,
    attachment: AttachmentId,
    registry_version: u64,
    tools: Vec<ToolDescriptor>,
    expires_at: Timestamp,
    signature: DeviceSignature,
}
```

The component registers only descriptors permitted by the signed projection and
its own supported protocol range.

## Agent boundary

The complete decision path is:

```text
ChatGPT chooses tool and arguments
    ↓
component performs deterministic validation and transport
    ↓
artistd executes exactly that tool
    ↓
component verifies and returns result
    ↓
ChatGPT chooses the next action
```

Forbidden paths include:

```text
component → local LLM → choose tool
artistd → local agent loop → reinterpret request
remote MCP service → cloud agent → route work
failed app-local tool → silently delegate to Artist's local agent
```

Artist's existing local agent remains a separate interface built on the same
runtime. It is not part of the ChatGPT web request path.

## Veilid transport

Artist uses Veilid rather than a hand-rolled libp2p/WebRTC network.

The browser component uses the Veilid browser/WASM implementation. Native
`artistd` uses the Rust Veilid core. Artist defines its own versioned application
protocol over Veilid primitives.

Veilid responsibilities include:

- node identity and encrypted routing;
- signed DHT records;
- private and safety routes;
- request/reply application calls;
- unsolicited application messages where needed;
- NAT traversal and reverse connectivity;
- direct connectivity where available;
- relay fallback where direct connectivity fails.

Artist protocol responsibilities above Veilid include:

- account, device, component, and attachment capabilities;
- canonical request serialization;
- end-to-end payload encryption to the destination device/session host;
- request IDs and nonces;
- replay windows and idempotency;
- signed results;
- command handles and output cursors;
- cancellation;
- registry projection and version negotiation;
- structured failure semantics.

Veilid routes bytes. `artistd` remains the authority.

## Identity model

Artist uses separate keys for separate authority domains.

### Account root key

The account root represents the user's Artist device family. It signs:

- device certificates;
- account roster updates;
- device revocations;
- component capabilities;
- recovery and key-rotation statements.

It remains under user control and has an explicit recovery mechanism.

### Device key

Each `artistd` installation has a device key certified by the account root. It
signs:

- Veilid/DHT advertisements;
- session-host records;
- registry projections;
- tool results;
- device-to-device synchronization messages.

### Component key

The component generates or accesses a signing key during pairing. The private
key is non-exportable where the host/browser platform permits it.

The account authorizes the public key with a component capability:

```rust
struct ComponentCapability {
    account: AccountPublicKey,
    component: ComponentPublicKey,
    permissions_ceiling: PermissionSet,
    issued_at: Timestamp,
    expires_at: Timestamp,
    component_release_policy: ComponentReleasePolicy,
    nonce_domain: NonceDomain,
    signature: AccountSignature,
}
```

An MCP bearer token, remote edge credential, bootstrap record, or Veilid node ID
alone can never authorize local execution.

### Attachment capability

Each ChatGPT conversation attaches through a capability scoped to a specific
Artist session and actor:

```rust
struct AttachmentCapability {
    account: AccountPublicKey,
    component: ComponentPublicKey,
    session: SessionId,
    actor: ActorId,
    permissions: PermissionSet,
    issued_at: Timestamp,
    expires_at: Timestamp,
    nonce_domain: NonceDomain,
    signature: AccountOrDeviceSignature,
}
```

The daemon remains the final policy enforcer for every call.

## Pairing and durable component state

Pairing occurs between the reviewed component and a user-controlled Artist
account. The remote MCP service is not an identity provider for local authority.

Representative flow:

```text
1. User enables the published Artist app.
2. User or model calls artist_open.
3. The reviewed component opens in an unpaired state.
4. The component discovers a local daemon or accepts a QR/pairing code.
5. Component and daemon perform authenticated key exchange.
6. The daemon shows one local approval with the durable permission ceiling.
7. The account signs a component capability.
8. The component stores its private key and signed capability in durable,
   installation-scoped storage.
9. Later conversations restore the capability without another approval.
```

The component requires a stable dedicated origin or equivalent platform storage
that survives:

- component teardown and remount;
- new ChatGPT conversations;
- browser restart;
- ChatGPT restart;
- ordinary cache churn;
- component updates compatible with the authorized release policy.

Clearing the installation's secure browser/app storage or moving to another
browser/device may require pairing that installation. Ordinary conversation
changes may not.

A user can revoke:

- one component installation;
- one device;
- one attachment;
- one session permission;
- an obsolete component release range;

without rotating unrelated credentials.

## Tool-call lifecycle

### 1. ChatGPT selects an app-local tool

After mounting, ChatGPT sees ordinary individually described tools, such as:

```text
artist_sessions_list
artist_session_attach
read
find
grep
edit
write
bash
```

These tools are supplied by the component, not by the remote MCP server.

### 2. The host invokes the component directly

The ChatGPT host sends the exact selected tool name and arguments to the
component through the MCP Apps host/app channel.

The remote MCP service receives no corresponding `tools/call` request.

### 3. The component resolves an explicit attachment

Conversation-local state identifies the active attachment. If none exists, the
component uses account-scoped discovery tools to list sessions and establish an
attachment before exposing workspace tools that require one.

No mutable connection-global "current session" is authoritative.

### 4. The component signs and encrypts the request

```rust
struct SignedToolRequest {
    protocol_version: u32,
    request_id: RequestId,
    attachment: AttachmentCapability,
    tool: String,
    arguments: serde_json::Value,
    issued_at: Timestamp,
    nonce: Nonce,
    idempotency: IdempotencyKey,
    signature: ComponentSignature,
}
```

The canonical envelope is encrypted to the destination device/session host
before entering Veilid.

### 5. Veilid delivers the request

The component sends a Veilid application call through an existing route, a newly
resolved private route, or relay fallback.

Mesh peers see only the routing and traffic metadata required by Veilid. They do
not see the Artist tool name, arguments, attachment, or result.

### 6. `artistd` verifies and dispatches

The daemon verifies:

- protocol version;
- account/device certificate chain;
- component capability and release policy;
- attachment signature, scope, and expiry;
- component request signature;
- nonce freshness;
- request and idempotency identities;
- permission policy;
- tool export policy;
- tool schema version;
- session ownership and current state.

Only then does it dispatch through the canonical Artist registry.

### 7. `artistd` signs the result

```rust
struct SignedToolResult {
    request_id: RequestId,
    idempotency: IdempotencyKey,
    session: SessionId,
    device: DeviceId,
    outcome: ToolOutcome,
    content: ToolResultContent,
    completed_at: Timestamp,
    signature: DeviceSignature,
}
```

### 8. The component verifies and returns

The component decrypts the response and rejects unsigned, mismatched, replayed,
stale, or incorrectly signed results. A valid result is converted directly to
an MCP `CallToolResult` and returned through the app-local tool callback.

The normal cycle is therefore:

```text
model → app-local tools/call → Veilid → artistd
      ← app-local tool result ← Veilid ← signed result
```

No follow-up-message workaround is required for ordinary tool calls.

## Sessions, attachments, and concurrency

An externally driven Artist session is distinct from a local model session.
ChatGPT owns model conversation history; Artist owns execution history and local
runtime state.

Each external conversation attachment receives:

- a stable actor ID;
- a private shell namespace;
- independent cancellation state;
- an independent permission mask;
- an active-request table;
- shared access to the session workspace;
- shared file/hashline coordination;
- recording in the session event log.

Example actor IDs:

```text
external/chatgpt/att_7Zd91
external/chatgpt/att_M2x8Q
external/other-client/att_K9r31
```

One daemon owns the event writer for a session. Multiple components or clients
coordinate through that runtime rather than opening competing event-log writers.

## Veilid discovery records

### Account roster

The account root signs a monotonically versioned roster:

```rust
struct AccountRoster {
    account: AccountPublicKey,
    sequence: u64,
    devices: Vec<DeviceRecord>,
    revoked_devices: Vec<DeviceId>,
    expires_at: Timestamp,
    signature: AccountSignature,
}
```

The roster is replicated among user devices and published as signed Veilid DHT
state. No central account table is authoritative.

### Device record

A device publishes short-lived signed reachability data:

```rust
struct DeviceRecord {
    account: AccountPublicKey,
    device: DeviceId,
    veilid_node: VeilidNodeId,
    route_record: RouteRecordRef,
    protocol_range: VersionRange,
    expires_at: Timestamp,
    signature: DeviceSignature,
}
```

### Session advertisement

Session discovery records are encrypted to authorized account/component keys and
contain only what is needed to choose a session:

```rust
struct SessionAdvertisement {
    account: AccountPublicKey,
    device: DeviceId,
    session: SessionId,
    display_label: String,
    capabilities: SessionCapabilities,
    expires_at: Timestamp,
    signature: DeviceSignature,
}
```

Workspace paths and private labels are not published in plaintext DHT records.

## Connectivity and relay behavior

Connection order is:

```text
existing authenticated Veilid route
→ newly resolved direct/private route
→ alternate route
→ encrypted Veilid relay/safety-route fallback
```

Relay fallback is part of the architecture. Direct-only networking is invalid
because it fails on restrictive NATs, managed networks, and mobile networks.

Bootstrap nodes assist a browser/WASM Veilid node in joining the overlay. They
are replaceable discovery infrastructure, not an authority root. The component's
reviewed CSP contains the stable origins required to bootstrap the Veilid node;
after joining, Artist payloads remain encrypted end to end.

## Shared edge federation

All conforming remote MCP edges serve the same bootstrap app identity and only
the `artist_open` tool.

An edge may be operated by an Artist user, organization, university, open-source
group, or the Artist project. Since sensitive tools are app-local, edge nodes do
not receive sensitive requests or results.

The edge federation requires:

- a shared public DNS name;
- replicated edge discovery or DNS records;
- short-lived TLS credential issuance for conforming nodes;
- health checking and removal of broken nodes;
- signed release and compatibility metadata;
- no shared long-lived wildcard private key.

Edges can still deny service or return malformed bootstrap data. The host and
component reject data that does not match the published app/resource identity.
Availability is recoverable by selecting another edge.

Relay participation is independent from HTTPS edge participation. A daemon may
host sessions, publish DHT records, relay Veilid traffic, provide remote MCP
bootstrap service, or perform any combination of those roles.

## Long-running work

Long-running commands return a local handle promptly:

```text
bash.exec("cargo test")
→ shell_id / command_id

bash.read_output(shell_id, command_id, cursor)
→ current output and next cursor
```

The app-local callback for command launch completes when the handle is created.
The command continues under `artistd`.

Output is cursor-based and batched. The component does not poll aggressively.
Where Veilid application messages are suitable, `artistd` may notify the
component that output or job state changed; the component still retrieves
bounded result chunks through an explicit tool call.

Cancellation is request-, command-, and attachment-scoped. Cancelling one
ChatGPT attachment cannot cancel another attachment's work.

## Failure behavior

Failures are structured and attributable:

- `artist_component_not_mounted`
- `component_unpaired`
- `component_storage_unavailable`
- `component_capability_expired`
- `component_release_unauthorized`
- `component_version_unsupported`
- `app_tool_registration_failed`
- `session_not_attached`
- `device_offline`
- `session_unavailable`
- `veilid_bootstrap_failed`
- `veilid_route_unavailable`
- `request_replayed`
- `permission_denied`
- `tool_not_exported`
- `tool_schema_mismatch`
- `registry_signature_invalid`
- `result_signature_invalid`
- `result_request_mismatch`

The system never silently reroutes a session to another workspace, silently
passes a sensitive request through the remote MCP service, or invokes the local
Artist agent as a fallback.

## Required conformance validation

The architecture is accepted only after integration tests establish all of the
following:

### Host/app tool behavior

- `artist_open` mounts the component;
- ChatGPT queries the component's app-side `tools/list`;
- tools with model visibility become available to the model;
- ChatGPT invokes app-side `tools/call` directly;
- remote MCP logs contain `artist_open` but no sensitive app-local invocation;
- app-local results return through the normal model/tool loop;
- tool-list-changed updates are reflected correctly;
- component teardown removes app-local tools and leaves `artist_open` usable;
- remount restores tools without leaking the pending sensitive invocation.

### Component integrity and storage

- the host rejects a component artifact whose digest or publication identity
  differs from the reviewed release;
- a malicious edge cannot substitute executable component bytes;
- pairing survives a new conversation;
- pairing survives component teardown/remount;
- pairing survives browser and ChatGPT restart;
- revocation invalidates restored component state;
- concurrent conversations cannot steal or confuse each other's attachment
  state.

### Veilid transport

- browser/WASM component to native Rust daemon application call;
- direct-route operation;
- relay/private-route-only operation;
- route loss and transparent re-resolution;
- duplicate delivery of a destructive request;
- delayed and replayed envelopes;
- malicious DHT, discovery, or relay records;
- ciphertext modification;
- daemon disconnect and reconnect without changing durable session identity.

### Runtime and permissions

- one shared app serving unrelated Artist accounts;
- two ChatGPT conversations attached to one session concurrently;
- one component switching between sessions on two devices;
- revoked component, device, and attachment capabilities;
- cancellation isolation;
- registry projection for native, extension, skill, imported MCP, and internal
  tools;
- mixed component/daemon protocol versions;
- hundreds of consecutive model/component/daemon tool cycles;
- long-running command handles and bounded output retrieval.

## Repository ownership boundaries

The architecture should converge on these responsibility boundaries:

```text
artist-runtime
  canonical tool registry
  session and attachment runtime
  permission filtering
  tool provenance/export policy

artist-agent
  local model loop
  Rig adapter
  TTSR and subagents
  not used in the ChatGPT web execution path

artist-protocol
  identities and capabilities
  request/result envelopes
  registry projection
  versioning and error taxonomy

artist-mesh
  Veilid integration
  signed DHT records
  private-route application protocol
  browser/native compatibility

artist-mcp
  artist_open only
  immutable component resource metadata
  bootstrap compatibility responses

artist-component
  reviewed ChatGPT component artifact
  app-local tool registration
  pairing and attachment state
  browser/WASM Veilid node
  signed request/result adapter

artist-daemon
  multi-session supervisor
  Veilid participation
  local tool dispatch
  event recording
  revocation and replay protection
```

Crate boundaries may differ where dependency pressure justifies it, but the
responsibility and trust boundaries must remain intact.

## Final architectural statement

Artist exposes one shared publishable ChatGPT app without operating a central
execution, routing, identity, repository, or shell service.

The shared remote MCP federation exposes only `artist_open` and an immutable
reviewed component. The mounted component becomes the model-visible MCP tool
provider for all sensitive Artist operations. ChatGPT calls those tools directly
through the host/app channel. The component signs and encrypts each exact call
and sends it through Veilid to the destination `artistd`. The daemon authorizes,
executes, records, and signs the result; the component verifies it and returns a
normal tool result to ChatGPT.

ChatGPT is the agent. The component is the tool provider and cryptographic
transport adapter. Veilid is the decentralized network. `artistd` is the local
authority and executor.
