# AIConnect pre-deployment acceptance

Target: Rust native CLI and local daemon, third-party iroh networking, a Cloudflare Workers website serving Skill/install distribution, real local Agent collaboration. Work stops before publication or deployment. A green local protocol test alone is not release readiness.

The base communication milestone (2026-10-29, before task confirmation) passed all **12 original gates** via `node scripts/verify-local.mjs --agents`; its exact artifact and reports remain in `docs/local-validation-2026-10-29.json` as historical evidence. The subsequent task-confirmation build passed 12 non-model regression gates plus a separately executed real two-Agent confirmation flow. Its current binary SHA-256, all regression checks and actual proposed → accepted → delivered → approved evidence are in `docs/task-confirmation-validation-2026-10-29.json`. The expanded `--agents` command now includes both real-Agent scenarios. No production deployment or publication was executed.

## Evidence matrix

Group-chat extension (2026-10-29): `docs/group-chat-validation-2026-10-29.json` records the latest artifact, **14 passing regression gates**, three real Agents communicating through the native group, actual relay-only group transport, and an additional live inbox-watch cursor test. This is the latest feature milestone; prior artifact fingerprints remain historical.

| Requirement | Authoritative evidence required | Current status |
|---|---|---|
| Native Rust executable; no AIConnect npm publication needed | Build output, executable version, startup on supported platform | Windows x64 debug executable ran successfully in native gate |
| Persistent identity and local history | Stop/start independent daemon, same ID and stored messages | Passed native gate |
| Cross-owner contacts, import/export/group/block | CLI tests with distinct identities; tampered card refusal; persistent block | Passed import, signed card export, group editing, block/unblock, signature tamper, removal ending sessions and denying reconnect |
| Temporary single-peer connection, expiry, revocation | Three real native endpoints; refused second identity, expired invite, post-close send | Passed, including 4 concurrent distinct-peer claim races |
| Reliable messages | Native send/ACK, receiver durable record, stable-ID dedup, conflict rejection | Passed native gate, including offline queue surviving both sender and receiver restart then ACK |
| Local IPC protection | Raw TCP absent/wrong token and malformed ingress; daemon remains healthy | Passed native gate; singleton and oversize audit separate |
| Execution separated from communication | Temporary task rejection; allowed/denied contact task tests | Passed task refusal, terminal rejection/nonrevival, wake/task independent flags; actual execution lab separate |
| Agent wake and real A/B collaboration | Two configured real Agent instances communicate through product CLI and modify only fixture workspaces | Passed actual Codex A/B lab: 2 native task/reply messages, A 6 tools / B 1 tool, plan verified from B fixture |
| Multi-Agent group and membership permissions | Three native endpoints, signed bound invitations, owner/admin/member checks, mute/remove/leave/dissolve, persistence and real Agents | Passed native tests, three real Codex Agents (3 tools each), group inbox watch and real relay transport; owner must be online for authorization |
| Forced relay works | Actual observed relayed path with direct connections unavailable | Passed lab with native iroh IP transports disabled: both endpoints selected direct=false, relay=true; uses actual public relay |
| Real heterogeneous NAT behavior (external follow-up) | Two physical or genuinely separate external networks | Outside local completion gate; explicitly unverified |
| Website runs on Workers locally | Wrangler local HTTP responses and tests, no deployment | Passed real local workerd HTTP, static route tests, security headers, MIME and no-chat API checks |
| Skill install from website | Real skills CLI discovery and isolated installation from local served URL | Passed official skills 1.7.4 discovery and project-scoped install; all 7 files match served bytes, including Codex adapter |
| Installers preserve identity and reject corrupted downloads | Local artifact fixtures and tests for correct/incorrect digests, reinstall, rollback | Passed actual signed native executable install/setup/reinstall, identity retention, invalid signature/hash/version, PowerShell bootstrap and uninstall |
| Supported development artifact | Windows x64 artifact built and smoke-tested on this machine | Windows x64 native development executable verified; production release not published; other platform workflows are preparedness, not verified support |
| No live changes | No actual deployment, npm/release publication, production DNS/auth mutations | Local-only commands and optional bundle dry-run; no deployment/publication performed |

## Independent native acceptance gate

```powershell
cargo build -p aiconnect-cli
node test/acceptance/run.mjs
# Or use a supplied release artifact:
node test/acceptance/run.mjs --binary <absolute-path-to-aiconnect>
# Include real Agent laboratory (requires local runtime authentication):
node test/acceptance/run.mjs --full
```

The gate creates new per-run state below `.local/acceptance/`, starts three distinct daemons, and cleans up only the daemons it starts. Invitations and IPC bearer credentials are never printed. `report.json` contains named checks and failures; a missing binary is a failed prerequisite, never a skipped success. The test runner neither implements peer transport nor forwards Agent messages.

Native Windows x64 execution on 2026-10-29 passed all 15 checks. Evidence: `.local/acceptance/2026-10-29T13-28-03-276Z/report.json`. This is local protocol evidence only. An earlier attempt found Windows child processes retaining inherited output pipe handles after the CLI had exited; it was explicitly stopped and cleaned up before retry. The runner now observes process exit, drains output, and closes inherited pipes rather than treating pipe lifetime as process liveness.

Additional reports inspected directly:

- `.local/lab-runs/2026-10-29T13-26-23-441Z-540d08/report.json`: actual public relay selected for both endpoints, native IP transport disabled. This does not test a real UDP firewall transition or heterogeneous NAT.
- `.local/lab-runs/2026-10-29T13-26-55-043Z-da3c25/report.json`: two real Codex Agents, A dispatches a task through native AIConnect to B, B reads its synthetic fixture and replies, A writes a verified migration plan. No test-orchestrator message forwarding.

The gate assumes the current JSON CLI contract. If a command contract changes, update assertions against the documented semantics rather than weakening them to accept absent evidence. Incoming task execution must be tested separately from message storage: a delivered chat is not proof an Agent ran.

## Isolation and scope

Separate data directories, process instances, and working directories are logical isolation. They are not an operating-system security boundary under a shared user account. Containers or separate restricted users are needed to make filesystem-denial claims. Same-host endpoints prove local protocol behavior, not external-network NAT traversal. A hosted relay is not an offline mailbox.

## Completion rule

Record actual command output and report paths for each matrix row. Do not mark local functionality tested while forced relay, real Agent execution, installation, or the Windows x64 artifact remain unverified. Cross-platform workflows may be prepared without claiming those artifacts supported; external heterogeneous NAT testing remains an explicit follow-up and does not block the local stopping point. No live deployment is needed or permitted for this objective; local Workers validation and prepared deployment configuration are the intended stopping point.
