# Two local Agent environments

The lab uses separate native AIConnect daemon processes, data directories, endpoint keys, SQLite stores, IPC endpoints, and Agent workspaces. It runs on one physical host under one OS user: this is logical separation, not an operating-system security sandbox. Docker is optional; the current Windows Docker engine was unavailable during initial inspection.

## Commands

After building the Rust CLI:

```powershell
cargo build -p aiconnect-cli
node --test lab/lib.test.mjs
node lab/run.mjs --binary target/debug/aiconnect.exe
node lab/run.mjs --relay-only --binary target/debug/aiconnect.exe
node lab/run.mjs --agents --binary target/debug/aiconnect.exe
```

On Linux/macOS omit `.exe`. `--output DIRECTORY` chooses a task-owned output location. The harness writes one JSON summary to stdout and persists `.local/lab-runs/<run>/report.json`. A failed or unavailable model turn is a failure, not an implicit pass. Existing reports remain available for inspection.

## What is tested

The protocol smoke starts A, B, and an unauthorized third endpoint C. It checks distinct endpoint identities, one-use signed invites, bidirectional actual P2P messages, stable-ID deduplication, denial of task execution in temporary sessions, and session revocation.

The `--agents` run additionally imports signed contact cards, configures synthetic fixture task permissions, attaches B's real Codex adapter, and starts Agent A as a separate real model turn. A sends a task through the native CLI, B receives it through its native daemon and reads its own `settings.json`, and B's response travels back over AIConnect. A polls its own message history and writes `migration-plan.json` in its own workspace. The harness verifies a randomly generated marker that was initially present only in B's fixture, the complete settings contents, the recorded task/reply pair, and that B's source fixture was unchanged. The supervisor never forwards either Agent's task or response.

The local Codex runtime uses its existing login itself. The harness does not read, print, or copy credential files. Agent processes receive dedicated working directories. Do not interpret that alone as prevention of same-user filesystem access; stronger adversarial tests require OS users, containers, or an actual sandbox.

## Evidence limits

Same-host direct paths do not prove cross-ISP NAT traversal. `--relay-only` disables native iroh IP transports and checks both actual sender status paths report relay=true and direct=false after real messages through the public relay. It exercises relay transport, not an actual firewall-induced switch from direct to relay. The relay run needs external network access and can fail when the public service is unavailable. Full lifecycle, outbox/restart, adversarial policy checks, and local IPC checks are owned by `node test/acceptance/run.mjs --binary <path>`.

All fixtures are synthetic. The lab performs no actual software migration, does not modify user Agent settings, and never deploys to Cloudflare or any other service.

## Verified local runs — 2026-10-29

| Run | Result | Evidence beneath `.local/lab-runs/` |
| --- | --- | --- |
| Native direct protocol | Passed | `2026-10-29T13-25-57-849Z-575af7/report.json` |
| Native public relay-only protocol | Passed; both selected paths relay=true, direct=false | `2026-10-29T13-26-23-441Z-540d08/report.json` |
| Two real Codex Agents | Passed; A used 6 tools, B used 1; B claimed exactly one task; A wrote the verified fixture migration plan | `2026-10-29T13-26-55-043Z-da3c25/report.json` |

The real collaboration step took approximately 66 seconds. A's generated plan is at `2026-10-29T13-26-55-043Z-da3c25/a/workspace/migration-plan.json`. All daemons started by these successful runs were stopped by the harness. Four helper tests also passed.

An earlier run (`2026-10-29T13-24-05-110Z-c9338b`) failed because the Node supervisor waited for descendant-held Windows output pipes after the CLI process had already exited. The supervisor now observes the command process's exit, drains output, and closes its pipe handles; a regression test reproduces that descendant-pipe condition. The original failure report remains intact.
