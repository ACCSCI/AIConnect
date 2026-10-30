# Rust core API contract

`aiconnect_core` exports:

```rust
pub struct Config { pub directory: std::path::PathBuf, pub direct_only: bool, pub relay_only: bool, pub relay_url: Option<String> }
pub struct Core; // internally clonable Arc state
impl Core {
  pub async fn open(config: Config) -> anyhow::Result<Self>;
  pub fn id(&self) -> String;
  pub async fn dispatch(&self, request: serde_json::Value) -> anyhow::Result<serde_json::Value>;
  pub async fn close(&self);
}
```

CLI daemon keeps one Core open. Local IPC forwards JSON requests, one response each. Errors are converted by CLI to `{error: message}`. Actions:

Group actions: `groups.create` (`name,maxMembers?,ownerOnline?`), `groups.replicate` (`groupId`, owner-only upgrade), `groups.list` (`offset?`), `groups.show/sync` (`groupId`), `groups.invite` (`groupId,peer,ttlSeconds?`), `groups.join` (`invite`), `groups.role` (`groupId,peer,role:admin|member`), `groups.policy` (`groupId,peer,canSend`), `groups.remove`, `groups.leave/dissolve`, `groups.send` (`groupId,text,id?`), `groups.messages` (`groupId,after?`), `groups.inbox` (`after?`). New groups default to `replication:members-v1`; legacy `ownerOnline:true` groups retain per-message owner checks. Replicated groups use locally verified owner snapshots and signed author logs, independent per-peer pull cursors, and offline replica catch-up. `groups.sync` returns peer results (one page each), `fresh:false` and `authorityOnlineRequired:false`; it does not claim global current authority. Message rows include signatureVerified, authorSequence, previous, epoch, recordRef and authorization (`current_epoch|historical_unfinalized|legacy_unsigned`). Chat never grants execution rights. See GROUP-REPLICATION.md for partition, epoch, new-member history and ACK limits.

Task confirmation is an additive v1 `task_event` message kind, separate from legacy automatic `task` execution. Actions: `tasks.propose` (`sessionId,text,title?,ttlSeconds?,contractId?,eventId?`), `tasks.accept`, `tasks.decline`, `tasks.deliver`, `tasks.approve`, `tasks.reject` (`taskId,eventId?`, text required except accept; optional sha256 on deliver), `tasks.list` (`offset?`), `tasks.show` (`taskId`), `tasks.history` (`taskId,after?`). Mutations return `{task,eventId,delivery}`. Task ID is the proposal event UUID. All events are persisted atomically with their message and projection, revision checked, role checked, and never auto-executed by the runtime. See TASK-CONFIRMATION.md for state, replay, deadline and evidence semantics.

- `{"action":"status"}` returns `{id, version, directOnly,lastPath:{peerId,direct,relay}|null}`. `lastPath` is observed selected transport after a completed RPC, not an estimate.
- `{"action":"identity","name":"optional display name"}` returns signed contact card object (also status identity when omitted name).
- `{"action":"contacts.list"}` => `{contacts:[...]}`.
- `{"action":"contacts.import","card":{...},"name":"optional local alias","group":"optional"}`.
- `{"action":"contacts.remove","id":"peer id"}`.
- `{"action":"contacts.export","id":"peer id"}` returns the original signed card.
- `{"action":"contacts.group","id":"peer id","group":"local group"}` changes local grouping.
- `{"action":"contacts.block","id":"peer id","blocked":true}`.
- `{"action":"contacts.policy","id":"peer id","chat":true,"wake":false,"task":false}`.
- `{"action":"temp.create","label":"name","ttlSeconds":1800}` => `{session:{...},invite:"..."}`.
- `{"action":"temp.join","invite":"..."}` => `{session:{...}}`.
- `{"action":"connect","id":"contact id"}` => `{session:{...}}` persistent direct-contact session.
- `{"action":"sessions"}` => `{sessions:[...]}` excludes capability and invitation secrets.
- `{"action":"send","sessionId":"...","text":"...","id":"optional stable UUID","kind":"text|task"}` => `{id,status:"acked|pending",error?:...}`. Pending means persisted for retry, not delivered.
- `{"action":"messages","sessionId":"...","after":0}` => `{messages:[...],nextAfter,hasMore}`.
- `{"action":"inbox","after":0}` => `{messages:[...],nextAfter,hasMore}` ordered numeric `sequence` cursor. Received messages only; include `kind`, `wakeAllowed`, `taskAllowed` for adapter gating. Public pages contain at most 32 messages and approximately 400 KB of encoded row data, below the local IPC response cap. Continue with `after=nextAfter`; an empty page preserves the input cursor.
- `{"action":"inbox.cursor"}` => `{cursor}` queries the latest received sequence directly without loading or transferring message history. Runtime attachment uses this tail cursor to avoid executing historical tasks.
- `{"action":"task.authorize","sessionId":"...","sender":"peer id"}` => `{allowed,wake,task,expiresAt}`. Rechecks current session and contact policy immediately before runtime execution. An expired, ended, blocked or removed session returns an error.
- `{"action":"session.close","sessionId":"..."}`.
- `{"action":"retry"}` retries pending outbox.

Default contact policy chat=true,wake=false,task=false. Temporary sessions only allow text, never task execution. Endpoint cards are self-signed; owner labels are declarations, not verified human identity. Runtime attachment is CLI-owned; core persists inbound messages and includes authorization metadata only.

Task messages have an absolute deadline derived from their sender's immutable `created` timestamp: `expiresAt = created + 60000`. Receiver rejects tasks already expired or created more than 30 seconds in the future. Inbox exposes this same deadline; the runtime must not restart it on receipt or retry. Policy fields are computed when reading, rather than permanently stored authorization grants. Runtime execution still requires the fresh `task.authorize` check.

Permanent remote denial (`permission_denied`, block, revocation, expiration, malformed message) moves an outgoing message to terminal `rejected` and returns an error. Only transient connectivity errors return `pending` and enter retry. Reusing the same message ID with different session, sender, text or kind is rejected. ACK follows a SQLite FULL-synchronous commit. A repeated acknowledged ID is idempotent.

`contacts.remove` also ends that peer's existing local sessions. `session.close` remains possible after block or policy revocation. Blocking applies to temporary sessions as well as persistent contacts. Joining temporary sessions is serialized with local close/block mutations so a concurrent join cannot reopen a revoked invite.

Explicit block, chat denial, session close, expiry, and contact removal terminal-cancel the corresponding pending outbox items. Re-enabling a contact does not revive those requests. Wake/task policy is inbound authorization and does not silently cancel unrelated outgoing messages.

Data-directory `endpoint-port` preserves the selected IPv4 UDP port across restart (failure to rebind is explicit). `direct_only` selects the minimal preset; `relay_only` removes all IP transports using iroh's native configuration. The modes are mutually exclusive. A Core caller explicitly selecting direct_only=false uses the n0 preset/custom relay; the CLI now defaults to direct_only=true and requires --dev-public-relay for public development relays. Custom CLI relays require a registered free-provider policy with purpose iroh-relay. No production-free relay is promised. The core ALPN is intentionally incompatible with the earlier Node prototype.

Core errors use short stable snake_case messages. All times Unix milliseconds. Data is isolated per directory. Rust protocol uses ALPN `aiconnect/rust/1`, explicitly separate from Node prototype.

SQLite `user_version=300` marks this native schema. The pre-native Node schema is detected before initialization writes and rejected with `incompatible_profile_use_new_data_directory`; no implicit migration or deletion is performed. Existing correctly shaped native schema version 0 is recognized and marked 300.

Group capacity policy: creation defaults to 16 and accepts an explicit maxMembers from 2 through 64, signed into the group snapshot. 32/64 are experimental after measured admission failures; old groups without maxMembers retain 16. Group snapshots alone use a bounded 44,000-byte signed payload verifier to support 64-node experiments; contact cards and other generic signed payloads retain the 24,000-byte limit. See GROUP-PRESSURE-2026-10-29.md for data and default decision.

## Six-layer local JSON actions

All actions below are authenticated local IPC requests. They are not public HTTP
endpoints. Persistent records remain in the profile SQLite database. Descriptors,
grants, domain evidence, channel content and external protocol observations have
different evidence levels; none implies automatic permission to execute or pay.

| Action | Request fields / meaning |
| --- | --- |
| `owner.init`, `owner.inspect` | Separate OwnerRoot account identity; group owner is unrelated. |
| `owner.pair-request` | Device-signed one-time pairing request. |
| `owner.pair` | `request,audience,scopes[],ttlSeconds?,maxCalls?`; root signs a validated pairing request. |
| `owner.grant` | `agentId,audience,scopes[],ttlSeconds?,maxCalls?`; explicit direct issuance. |
| `owner.import` | `grant,expectedOwner,trustOwner:true`; pin owner explicitly. |
| `owner.verify`, `owner.consume` | `grantId,audience,scope`; consume also requires `nonce`. |
| `owner.revoke` | `grantId`; produce signed revocation evidence. |
| `owner.revocation-export` | Export bounded fresh revocation document. |
| `owner.revocations` | `document`; import root-signed revocations. |
| `owner.rotate`, `owner.rotation-import` | Root transition, or import `transition`. |
| `owner.backup`, `owner.restore` | Backup returns encrypted `backup` envelope; restore `backup,expectedOwner` without overwriting a root. Windows DPAPI is normally same-account/machine; Unix unsupported rather than plaintext export. |
| `owner.domain-challenge` | `domain`; returns document and HTTPS publishing URL. |
| `owner.domain-verify` | `domain,expectedOwner`; controlled HTTPS fetch, no silent owner repin. |
| `owner.domain-inspect` | `domain`; local evidence, not legal identity proof. |
| `profile.set` | `visibility:private\|lan\|public`; default private. |
| `capability.publish` | `skills[],visibility?,ttlSeconds?`; sign locally, return `descriptor`. |
| `capability.inspect` | Inspect local descriptor. |
| `capability.validate`, `capability.dry-run` | `descriptor?`, optional `skillId,input`; validate signatures/schema without execution. |
| `directory.add` | `id?,url,free:true,termsAllowed:true,termsUrl,ttlSeconds?`. |
| `directory.list`, `directory.remove`, `directory.sync` | Remove/sync by `id`; absent sync ID iterates registered sources. |
| `directory.import` | `sourceId?,directory:{schemaVersion:1,descriptors:[]},trusted?`; signed metadata only. |
| `discover` | `scope:mine\|trusted\|public\|all,query?,capability?,language?,os?,includeExpired?,limit?`; verified local cache. |
| `discovery.lan` | `mode:status\|advertise\|browse\|stop,seconds?,ttlSeconds?`; explicit LAN/public presence. |
| `discovery.relay.add/list/remove/probe` | Nostr source registration/probing; add `url,free:true,termsAllowed:true,termsUrl`, other mutations `id`. Not iroh relay selection. |
| `discovery.nostr.export/publish` | `confirmPublic:true`; publish requires source `id` and relay ACK. |
| `discovery.nostr.import/read` | Import `event,sourceId?`, or read relay `id,limit?`. Distinct Nostr key binding, not private chat. |

Capability envelopes use versioned canonical signed payloads. Input/output schemas
do not fetch remote references. Signature validity proves the signing key, not the
truth of self-declared capabilities, prices or names. Directory conflicts/expiry
remain visible; a directory cannot grant execution rights.

| Channel action | Request fields / meaning |
| --- | --- |
| `channels.create` | `name,visibility:private\|public,maxMembers?`. |
| `channels.list/show` | Show `channelId`; private channels reuse signed replicated groups. |
| `channels.invite/join` | Invite `channelId,peer`; join `invite`; reader defaults do not grant publishing. |
| `channels.publisher` | `channelId,peer,canPublish`; owner-controlled group publisher ACL. |
| `channels.publish` | `channelId,text,id?`; signed event and persistent delivery. |
| `channels.export` | `channelId`; returns `files` mapping `channel.json` and `feed.json` to strings; does not deploy. |
| `channels.subscribe` | `url` or `channelId`, optional `sourceId,expectedPublisher,wake:false`. |
| `channels.subscriptions/sync` | List subscriptions, or sync `sourceId`; bounded conditional fetch/backoff. |
| `channels.import` | `sourceId,body`; local feed/manifest input remains unverified transport provenance. |
| `channels.read` | `sourceId` or `channelId`, optional `after`; cursor-based replay. |
| `channels.notifications` | `after?`; bounded `{notifications:[{cursor,sourceId,event}],cursor,hasMore,executed:false}`. |
| `channels.wake` | `sourceId,wake`; explicit local read-only runtime notification opt-in, default false. |
| `channels.unsubscribe/mute` | `sourceId`, optional `muted`; active/muted affects notifications. |

Public manifests retain a bounded window and report gaps. JSON Feed mirrors and
ordinary external feeds are not native author-signed evidence. Notifications do not
invoke an Agent by default. Explicit local wake opt-in requires a read-only runtime,
independent budget/claim and fresh subscription/block checks; it does not execute
channel tasks or send automatic channel replies. See CHANNELS.md for cursor details.

| Service action | Request fields / meaning |
| --- | --- |
| `service.inspect/quote` | `provider?,capability?,declaredPrice?`; declared price is not provider authentication. |
| `service.invoke` | `transport:native\|a2a,provider?,capability,input,declaredPrice:{amount:0,currency:USD},grantId,audience,idempotencyKey,ttlSeconds?`; native needs `sessionId`. |
| `service.status/result/cancel` | `jobId`; cancellation best effort. |
| `service.review` | `jobId,approve,text?`; external completed remains delivered until local review. |
| `service.list/pending` | `offset?,transport?`; pending supports the optional local adapter. |
| `service.lookup/update` | Local adapter task mapping and state updates; privileged authenticated local IPC, not an unauthenticated remote API. |

Grant scope is `service:CAPABILITY_ID`; unknown/nonzero prices are denied. Job input
and idempotency keys are persisted, and changed-payload replay is rejected. Core is
the state source; adapters do not introduce a central task database. Native task
evidence and external HTTP observations remain distinguishable. No exactly-once or
automatic payment/settlement guarantee is made.

| Governance action | Request fields / meaning |
| --- | --- |
| `policy.inspect/set` | Set `policy` object of known bounded limits. |
| `policy.import/adopt` | Import `policy,label?` as proposal; adopt `proposalId` explicitly. |
| `budget.inspect/reserve` | Reserve `peer,owner?,resourceAction:execute,requestId,conversationId?`. |
| `budget.commit/release` | `reservationId`; restart does not reset counters. |
| `audit.export` | `after?,limit?` capped at 1000. |
| `provider.add` | `url,free:true,termsAllowed:true,termsUrl,purpose,ttlSeconds?`; local free-use attestation. |
| `provider.list/check/authorize` | Check/authorize `url`; check can record `statusCode:402\|429` or `free:false`. |
| `contacts.request` | `card`; send an unknown contact request without importing or granting wake/task. |
| `inbox.list/decide` | Unknown-request box; decide `id,decision:allow\|deny`. |

Provider checks are local eligibility gates, not a claim of externally verified
vendor terms. Controlled HTTP fetching rejects non-HTTPS, private/reserved DNS
answers, rebinding, credentials, disallowed ports/redirects and oversized bodies.
402/429 or expired eligibility disables a provider rather than billing the project.
No paid relay credentials, subscription or credential broker is supplied.

Budgets, rates, per-conversation turns, download bounds and audits apply at Core
boundaries. Local IPC credentials remain fully privileged on that profile; this
does not isolate malicious programs already running as the same OS user.

`network.diagnostics` is a read-only local transport/configuration report with
observed path state, not a WAN guarantee. `network.probe {peer}` explicitly checks
an existing contact without creating a session or chat message. It never purchases
or registers a provider, and cannot establish worldwide NAT coverage from one pair.
