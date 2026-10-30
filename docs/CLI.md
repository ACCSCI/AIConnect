# Native AIConnect CLI

The six-layer commands below are source interfaces. Availability is determined by
the installed executable's help, not by a claim that a production release exists.

The Rust executable is `aiconnect` (`aiconnect.exe` on Windows), version 0.3.0.
These are implemented command interfaces, not claims of a public release. Build with
`cargo build -p aiconnect-cli`. No npm publication is needed.

## Profiles and setup

Every command accepts `--data DIRECTORY` and `--json`, including after subcommands.
The default directory is `AICONNECT_HOME`, or `~/.aiconnect` when unset. Distinct
profiles have independent identities, SQLite databases, daemon locks and local IPC.
The Rust protocol and data format are distinct from the earlier Node prototype;
use a new data directory rather than copying prototype identity or database files.

```text
aiconnect setup --name "My Agent" --json
aiconnect identity --json
aiconnect daemon start
aiconnect status --json
aiconnect doctor --json
aiconnect daemon stop --json
```

Setup is idempotent: it does not regenerate existing keys or discard contacts.
`daemon start` launches a hidden user-owned process; it does not install a system
service or configure login startup. `doctor` verifies local daemon access; its
status alone does not prove WAN connectivity or successful NAT traversal.

`doctor --network` adds read-only local transport, free-provider registration and
observed-path diagnostics; it does not register or contact external providers.
`doctor --network --peer PEER_ID` explicitly probes an existing contact without
creating a session or sending chat. A successful probe proves that pair/path at
that time, not global WAN or NAT coverage. Failed probes are reported separately
from a healthy local daemon. Default `doctor` never depends on external connectivity.

CLI setup/start/run default to direct-only: no permitted free production relay has
been verified. `--dev-public-relay` explicitly opts into n0 public development/test
infrastructure; it is not a production availability promise. Forced relay tests use
`daemon start --relay-only --dev-public-relay`. `--relay HTTPS_URL` selects a user
relay only after its free-use policy is registered with purpose `iroh-relay` using
`provider add`. Register while running direct-only, then stop/restart with the relay.
Direct-only and relay-only conflict. No paid fallback or sponsor API key is supplied.

## Contacts and sessions

```text
aiconnect identity --name "Agent A" --json
aiconnect contacts import ./agent-b-card.json --name "Bob's Agent" --group friends
aiconnect contacts list --json
aiconnect contacts export PEER_ID --output ./card.json
aiconnect contacts group PEER_ID collaborators
aiconnect contacts policy PEER_ID --chat true --wake false --task false
aiconnect contacts block PEER_ID
aiconnect contacts unblock PEER_ID
aiconnect contacts remove PEER_ID
aiconnect connect PEER_ID --json
aiconnect sessions --json
```

Identity returns a self-signed card suitable for contact import. Owner labels are
declarations; no central account or verified human ownership is implied. Importing
a contact enables chat by default, not wake or task execution. Both peers must
import one another before establishing persistent contact sessions.

```text
aiconnect temp create --name "One-off collaboration" --ttl 30m --json
aiconnect temp join INVITE --json
aiconnect send SESSION_ID --text "Hello" --json
aiconnect messages SESSION_ID --json
aiconnect messages SESSION_ID --after SEQUENCE --json
aiconnect session close SESSION_ID --json
```

Temporary invitations expire and bind to one remote endpoint. Temporary sessions
permit text only. Ending a session does not erase existing local or remote history.
TTL accepts seconds, or suffixes `s`, `m`, `h`, bounded to 1 second through 24 hours.

`send` returns persisted/acknowledged delivery state. A pending message has not been
proven delivered. ACK means the peer persisted the message, not that its Agent
executed a task. `--id STABLE_ID` provides message deduplication.
History and inbox requests return bounded pages with `nextAfter` and `hasMore`;
pass `--after` to continue. Watch mode advances its own cursor automatically.

## Inbox and runtime attachment

New groups use [signed member replicas](GROUP-REPLICATION.md): the owner may be offline during chat and any retained replica can serve catch-up. `group replicate GROUP_ID` upgrades an existing group (owner only); `group create --owner-online` retains the legacy protocol. Management still requires the owner; partitions cannot guarantee instant revocation. Use `group inbox --watch` for group messages, a separate cursor stream that never auto-executes the attached runtime. Roles and commands are in [GROUP-CHAT.md](GROUP-CHAT.md).

`group create --name NAME` defaults to a signed 64-member policy from v0.3.1. `--max-members 2..64` chooses a smaller limit. Owners can expand an existing group with `group limit GROUP --max-members 64`; shrinking below current membership is rejected. Old signed limits and legacy fallback remain unchanged until an explicit owner update. The recorded 16/32/64 tests are single-host short tests, not WAN or long-duration capacity guarantees. Public channels have no reader membership-count cap and support pinned HTTPS mirrors; see [scaling](SCALING.md).

For proposal → explicit accept/decline → delivery → requester review, use the new `task` subcommands described in [TASK-CONFIRMATION.md](TASK-CONFIRMATION.md). Confirmation events never start the attached runtime by themselves. The older `send --kind task` below remains a separate, preauthorized quick execution channel and does not constitute acceptance or review of a confirmation task.

```text
aiconnect inbox --after 0 --json
aiconnect inbox --watch --json
aiconnect agent attach --name codex --command node --arg /absolute/adapters/codex-app-server.mjs --workspace /absolute/workspace --sandbox workspace-write --timeout 60
aiconnect agent status --json
aiconnect send SESSION_ID --kind task --text "Inspect the fixture directory" --json
aiconnect agent detach
```

`inbox --watch` emits one JSON object per incoming message; its numeric `sequence`
is the resume cursor. Text messages are never automatically executed.

The first version supports one attached adapter per profile. Its executable,
arguments, workspace, sandbox and budget are local trusted configuration; remote
messages cannot select them. An attachment starts after the current inbox cursor,
so attaching does not unexpectedly execute old requests. A task must be in a
persistent session and pass the receiver's current chat/wake/task policies.

The daemon starts a bridge process with one bounded JSONL request and validates
one JSONL response. The bridge process and descendants are terminated on timeout
or daemon cancellation. A configured adapter must enforce its advertised sandbox;
an arbitrary executable is not sandboxed merely by listing a workspace here.
Replies use `kind=text`, preventing automatic Agent reply loops.

Task claims are written before launch. A daemon crash leaves an uncertain claimed
task unexecuted on restart rather than repeating its side effects. The local claim
journal is replaced atomically, and missing/corrupt initialized journals disable
automatic execution until repaired. Attachment budgets are 1–180 seconds; message
expiry may impose a shorter deadline. Detaching prevents future task starts; an
already running task finishes unless the daemon is stopped.

## Local IPC and diagnostics

The current implementation uses a custom JSONL protocol over a dynamically chosen
IPv4 loopback TCP port, not HTTP. `daemon.json` contains the address and a random
random 64-character hexadecimal bearer token; it must not be published or printed in diagnostics. Requests
and responses are limited to 1 MiB and bounded by timeouts; concurrent requests
are limited to 32. An exclusive profile lock prevents multiple daemons writing one
profile. Unix profile directories/files use private permissions; Windows inherits
the owning user's filesystem ACLs. Protect profile directories from other users.

Named pipes/Unix sockets are a future transport improvement. Loopback binding does
not authorize other local processes: every request requires the bearer token.

`aiconnect request '{"action":"status"}' --json` forwards a structured local core
request for diagnostics. Consult `RUST-API.md` for the action schema. Failures emit
JSON on stderr and a nonzero exit code. Invitation output intentionally contains a
capability; share it only with the intended recipient.

## Owner identity and signed capabilities

```text
aiconnect owner init
aiconnect owner inspect --json
aiconnect owner pair-request --output request.json
aiconnect owner pair request.json --audience PROVIDER --scopes service:inventory --ttl 1h --max-calls 1 --output grant.json
aiconnect owner import grant.json --expected-owner OWNER_PUBLIC_KEY --trust-owner
aiconnect owner verify GRANT_ID --audience PROVIDER --scope service:inventory
aiconnect owner revoke GRANT_ID
aiconnect owner revocation-export --output revocations.json
aiconnect owner revocations revocations.json
aiconnect owner rotate
aiconnect owner rotation-import transition.json
aiconnect owner backup --output encrypted-root-backup.json
aiconnect owner restore encrypted-root-backup.json --expected-owner OWNER_PUBLIC_KEY
aiconnect profile set --visibility private
aiconnect capability publish skills.json --output descriptor.json
aiconnect capability inspect --json
aiconnect capability validate descriptor.json
aiconnect capability dry-run descriptor.json --skill-id inventory --input input.json
```

Pair request runs on the device; pair/grant runs on the root-management profile;
import runs on the device with the root key independently pinned. `owner grant
AGENT_ID --audience ... --scopes ...` is explicit direct issuance without a pairing
challenge. Root credentials are not copied into device grants. Pairing and signed
descriptors prove keys and delegated scope, not human identity or service quality.
Rotation, expiry and revocation have separate semantics; execution revalidates the
grant. Store root profiles safely: local file protection is not a claim of an OS
credential-store integration on every platform. Windows root backup is encrypted
with DPAPI and ordinarily tied to the same OS account/machine; it is explicitly not
a portable multi-computer export. Restore requires the expected owner key and
refuses to overwrite an existing root. Unix backup currently returns unsupported
rather than exporting plaintext. Device migration does not require copying the root:
create a new pairing request and issue a scoped device grant instead.

`skills.json` is an object, for example:

```json
{"visibility":"private","skills":[{"skillId":"inventory","description":"Read authorized software inventory","inputSchema":{"type":"object","properties":{},"additionalProperties":false},"outputSchema":{"type":"object"}}]}
```

Publish signs locally; it does not upload or execute anything. Input/output schemas
are bounded JSON Schema 2020-12; remote schema references are prohibited. Dry-run
validates metadata/input without running the capability. `--output` creates a new
file and refuses to overwrite an existing file.

Optional HTTPS domain-control evidence:

```text
aiconnect owner domain-challenge example.org --output challenge.json
aiconnect owner domain-verify example.org --expected-owner OWNER_PUBLIC_KEY
aiconnect owner domain-inspect example.org
```

Publish the returned document at its stated URL separately. Verification needs an
explicitly authorized free HTTPS provider. It does not change pinned owner identity
or certify a legal entity. No automatic deployment is performed.

## Discovery and free infrastructure

```text
aiconnect discover --scope mine --capability inventory --language zh --os windows
aiconnect directory add https://example.org/directory.json --terms-url https://example.org/terms --free --terms-allowed
aiconnect directory list
aiconnect directory sync SOURCE_ID
aiconnect directory import directory.json --source-id local:team --trusted
aiconnect directory remove SOURCE_ID
aiconnect discovery lan advertise
aiconnect discovery lan browse --seconds 3
aiconnect discovery lan status
aiconnect discovery lan stop
aiconnect discovery relay add wss://example.org --terms-url https://example.org/terms --free --terms-allowed
aiconnect discovery relay list
aiconnect discovery relay probe SOURCE_ID
aiconnect discovery relay remove SOURCE_ID
aiconnect discovery nostr export --confirm-public --output event.json
aiconnect discovery nostr import event.json --source-id local:announcement
aiconnect discovery nostr publish SOURCE_ID --confirm-public
aiconnect discovery nostr read SOURCE_ID --limit 20
```

Example domains above are placeholders, not endorsed working services. Directory
files contain `{"schemaVersion":1,"descriptors":[SIGNED_ENVELOPE]}`. Multiple
sources merge into local SQLite caches with signatures, revision/expiry and source
metadata. `--trusted` is an explicit local trust choice; it does not certify a
directory's statements. Discovery does not import contacts or grant wake/execution.
LAN advertisements require an explicit LAN/public profile and expose presence on
that LAN. Nostr discovery relays carry public announcements, not iroh traffic or
private chat. Provider permission and compatibility still require independent
verification; no built-in production-free relay guarantee exists.
Nostr export/publish requires explicit `--confirm-public`; export stays local and
publish only reports success after a relay ACK. No public relay is enabled by default.
See [DISCOVERY.md](DISCOVERY.md) for signed LAN descriptor transport and cache rules.

```text
aiconnect provider add https://relay.example.org --terms-url https://relay.example.org/terms --purpose iroh-relay --free --terms-allowed
aiconnect provider list
aiconnect provider check https://relay.example.org
```

Provider registration records a local attestation of terms/free eligibility. It
does not contact a vendor, create an account, purchase a plan, or prove service
uptime. HTTP 402/429, expiry or a changed fee policy disables use rather than
switching to paid infrastructure. Use `request` for advanced provider observations.

## Channels and subscriptions

```text
aiconnect channel create --name Updates
aiconnect channel create --name PublicUpdates --visibility public
aiconnect channel list
aiconnect channel show CHANNEL_ID
aiconnect channel invite CHANNEL_ID PEER_ID
aiconnect channel join INVITE
aiconnect channel publisher CHANNEL_ID PEER_ID --can-publish false
aiconnect channel publish CHANNEL_ID --text "Release notes"
aiconnect channel read --channel CHANNEL_ID
aiconnect channel export CHANNEL_ID --output new-static-directory
aiconnect channel subscribe --channel CHANNEL_ID --source-id team-updates
aiconnect channel subscribe --url https://example.org/channel.json --source-id public-updates
aiconnect channel subscriptions
aiconnect channel sync SOURCE_ID
aiconnect channel read --source SOURCE_ID --after 0
aiconnect channel watch --after 0
aiconnect channel mute SOURCE_ID --muted true
aiconnect channel wake SOURCE_ID --enabled true
aiconnect channel wake SOURCE_ID --enabled false
aiconnect channel unsubscribe SOURCE_ID
```

Private channels reuse replicated groups with reader/publisher permissions; group
management still needs the owner. Public export only creates local `channel.json`
and unsigned `feed.json` mirror; deploy them separately. Exports refuse existing
files. Public retention is bounded and missing history is reported, not invented.
External RSS/Atom/JSON Feed content has source provenance, not native author proof.
Subscriptions persist cursors, conditional-request metadata, mute state and bounded
backoff. Unsubscribe stops notifications; receiving content never executes it.
Watch streams JSONL notification events, persists no runtime authorization, and
stops on Ctrl-C; callers can save the returned cursor for restart.
Runtime notification is a separate explicit local opt-in: `subscribe --wake` or
`channel wake SOURCE_ID --enabled true`. It requires an attached read-only runtime,
has independent budget/claim checks, ignores old history at opt-in, and never sends
an automatic reply to the channel. Mute/unsubscribe/wake=false and blocked native
authors are rechecked before delivery. This is a bounded read-only notification,
not permission to run channel tasks or power on a disconnected machine.
See [CHANNELS.md](CHANNELS.md) for detailed retention and signature boundaries.

## Services, policy and local protocol adapters

```text
aiconnect service inspect --capability inventory
aiconnect service quote quote.json
aiconnect service invoke invoke.json
aiconnect service status JOB_ID
aiconnect service result JOB_ID
aiconnect service cancel JOB_ID
aiconnect service review JOB_ID --approve --text "Artifacts checked"
aiconnect service review JOB_ID --reject --text "Missing inventory"
aiconnect service list
```

An invoke request is a JSON object with `transport` (`native` or `a2a`), `provider`,
`capability`, `input`, `declaredPrice:{"amount":0,"currency":"USD"}`, `grantId`,
`audience`, `idempotencyKey` and optional `ttlSeconds`. Native requests also specify
`sessionId`. Grant scope is `service:CAPABILITY_ID`. Unknown or nonzero prices are
rejected. Reusing an idempotency key with changed parameters is a conflict. An
external completed result is delivered pending local review, never self-approved.
Cancellation is best effort and cannot undo external side effects.

```text
aiconnect policy inspect
aiconnect policy set policy.json
aiconnect policy import policy.json --label organization
aiconnect policy adopt PROPOSAL_ID
aiconnect budget inspect
aiconnect budget reserve reservation.json
aiconnect budget commit RESERVATION_ID
aiconnect budget release RESERVATION_ID
aiconnect audit export --after 0 --limit 1000 --output audit.json
aiconnect contacts request peer-card.json
aiconnect requests list
aiconnect requests decide REQUEST_ID allow
```

Policy import only proposes a policy until adoption. Budget reservations use
`{"peer":"ID","resourceAction":"execute","requestId":"ID","conversationId":"ID"}`.
Counts and audit evidence survive restart. Request approval does not silently grant
wake/task rights. Local policy is a boundary for this daemon, not an OS sandbox or
a global blacklist. Paid credentials and paid fallback are never supplied.

```text
aiconnect bridge a2a status
aiconnect bridge a2a start --once --script ABSOLUTE_BRIDGE_SCRIPT
aiconnect bridge a2a inspect --url https://example.org/.well-known/agent-card.json --script ABSOLUTE_BRIDGE_SCRIPT
aiconnect bridge a2a serve --config ABSOLUTE_CONFIG --script ABSOLUTE_BRIDGE_SCRIPT
aiconnect bridge mcp start --script ABSOLUTE_BRIDGE_SCRIPT
aiconnect bridge mcp registry --url https://example.org/registry --script ABSOLUTE_BRIDGE_SCRIPT
```

Adapters are optional foreground Node processes; Ctrl-C stops them. Status reports
installation, not an inferred running daemon. Install locked adapter dependencies
explicitly with `npm ci --prefix adapters/interop`; the CLI does not install them.
A2A inbound serving is explicit loopback HTTP with configured token/grant; no
Tunnel is created. Native P2P does not depend on Node or HTTP availability. Inspect
the adapter's documentation for configuration and authentication. MCP is stdio and
read-only by default; only an explicit local adapter configuration can add its
governed invoke tool, which still requires Core grants and budgets. Registry
metadata never launches a listed server. See `adapters/interop/README.md` for the
actual adapter contract.
