# AIConnect local implementation security review

Review date: 2026-10-29. Scope: native Rust client, local daemon, agent adapter, installer, and local Cloudflare Worker site. This is a development review, not a claim of independent certification or production readiness. No production deployment is authorized by this review.

## Required invariants

| Boundary | Required evidence before release |
| --- | --- |
| Endpoint identity | Keys persist after restart; contact-card signature verifies the endpoint key. Owner name remains an unverified declaration unless separately bound. |
| Temporary invitation | Signature validation, bounded expiry, exactly one admitted peer under concurrent joins, remote endpoint matches ticket/card, revocation persists after restart. |
| Contacts and policy | Blocking affects already-open sessions; contact removal has defined access semantics; permission edits affect queued tasks before execution. |
| Message delivery | ACK follows SQLite commit; duplicate IDs must match session, sender, message kind, and content. Retried tasks do not produce multiple executions. |
| Local IPC | Loopback-only or local socket; unpredictable instance authentication; bounded request size and timeouts; one daemon per data directory. Local OS account remains a trust boundary. |
| Agent execution | Only an explicit task is executable; configured local runtime, fixed workspace and arguments; expiry, limits, durable execution claim, current policy checked immediately before spawn. Chat text is never shell input. |
| Installation | Pinned release key, signed manifest, artifact digest/size and version check before replacement; failed updates preserve old binary and all user data. Unsigned fixtures accepted only in explicit localhost test mode. |
| Worker site | Static discovery/docs/downloads only; no private state, keys, account tokens or relay service. Tests target local Worker without deployment. |

## Review observations

- `docs/RUST-API.md` explicitly distinguishes chat, wake and task permission. Temporary sessions are text-only and owner labels are declarations. This is a suitable initial contract; implementation and adversarial tests remain required.
- Existing `runtime/peer.mjs` Node prototype binds invitations to the first endpoint and checks both session capability and authenticated remote endpoint. `runtime/store.mjs` commits message inserts before the caller sends ACK. Those behaviors should be retained in Rust.
- The Node prototype is not proof of the new native implementation. In particular, `runtime/daemon.mjs` has no data-directory singleton and no contact/task policies. The Rust daemon is being developed with its own lock and local IPC.
- `distribution/install.mjs` initially enforces an Ed25519 manifest signature, bounded artifact size, SHA-256, executable version smoke test, staged replacement and rollback. Production key absence fails closed. Bootstrap code itself must still be obtained through a trusted channel; signing the downloaded executable does not authenticate a compromised installer.

## Pending independent checks

The implementations are still being written. Findings below must be updated from actual code and executable tests before the review is treated as complete:

1. Invite admission concurrency, end/restart/rejoin, block/restart/send.
2. Queued task then policy revoke then adapter attach: no execution.
3. Duplicate task and restart: no duplicate side effect; interrupted task reconciliation must avoid claiming impossible exactly-once guarantees.
4. Wrong IPC token, oversized/slow request, singleton contention, stale daemon metadata.
5. Invalid manifest signature/hash/version and failed update leave previous install and identity untouched.
6. Local Worker route tests and artifact source review; no deployment action.

Process-only A/B work directories are logical isolation, not OS access isolation. Same-host direct connectivity does not establish WAN NAT traversal or relay-only operation.

## Findings fixed during development

The reviewer reported the following code defects to the implementation owners. Current source contains fixes; the native acceptance gate and dedicated tests provide the evidence rather than the initial implementation:

- Invitation admission originally used separate load/save locks. A shared receive/mutation guard now serializes concurrent joins, local session close, contact removal and policy changes. Native acceptance exercises simultaneous claims by distinct endpoints.
- Contact removal originally deleted policy while leaving live sessions authorized by the unknown-peer chat default. Removal now ends contact sessions, and persistent sessions require a current contact.
- Message queries originally limited rows before filtering, starving late incoming messages and making sends fail after the first 1000 stored rows. SQL filtering now precedes retrieval.
- Permanent remote policy/session rejections originally stayed pending and could silently revive after policy changes. They now become rejected; task age is bounded and exact expiry reaches the adapter.
- Local session close originally required active chat permission. It can now end a blocked or otherwise inactive local session.
- Agent task authorization errors originally stopped inbox progress on ended/blocked sessions. They now record a denial and advance the cursor.
- Claim journal replacement originally removed the previous file before rename. It now uses atomic replacement; missing/corrupt initialized journals disable execution rather than replaying claims.
- Adapter executable selection now resolves to an existing absolute path at attachment time, avoiding working-directory/PATH substitution at execution time.

## Independently executed evidence

`node --test audit/ipc-negative.test.mjs` passed on the native Windows x64 executable: a second foreground daemon exited with code 1 without harness termination; an oversized frame was rejected; incomplete clients did not prevent authenticated status; daemon shutdown completed. Latest measured run: 1 test passed, approximately 1.5 seconds total. The harness observes process exit, not inherited Windows pipe lifetime.

`node --test distribution/test-install.mjs adapters/test/adapter.test.mjs` passed all 6 tests at the review checkpoint. These include valid signed installation/reinstall, signature/hash/version rejection preserving prior files, adapter input validation, approval refusal, malformed runtime output, and deadline cancellation. Newer expanded installer and runtime-policy tests are owned by their respective agents and need their final results recorded separately.

The final integrated verification also passed 9 adapter tests, 2 actual native installer tests and 7 native runtime-policy tests. The latter prove that chat does not execute, pre-attachment history does not replay, receipt-time authorization is rechecked before execution, denied tasks do not starve later authorized tasks, duplicate IDs/restarts do not relaunch a claimed task, and missing/corrupt initialized journals disable execution. Windows background startup uses no inherited parent capture handles; the detached child opens its own log while foreground diagnostics remain on stderr. Strict captured setup smoke tests complete in well under 8 seconds.

## Remaining practical limits

- On Windows, Unix `0600`/`0700` permission modes do not establish ACL isolation. Default per-user application directories inherit the account's protections; a user-selected shared data directory must have suitable Windows ACLs. Loopback bearer IPC protects against unauthenticated network callers, not another process able to read that account's private files.
- The Codex adapter's configured working directory and write sandbox are not a guarantee that reads outside that directory are blocked. A separate restricted OS user/container is required before granting untrusted peers task execution on a host containing accessible secrets. Owner labels remain unverified declarations.
- Claim-before-execute provides at-most-once automatic launch after restart. An interrupted task can remain uncertain and require manual reconciliation; this is not an exactly-once side-effect guarantee.
- Production signing keys and publication are intentionally absent at this local stopping point. The Worker is an installation/documentation site, not a message relay or offline mailbox.
