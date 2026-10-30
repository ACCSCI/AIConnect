# Agent runtime bridge

AIConnect keeps its network, identity, persistence and CLI in Rust. The first optional model adapter is `adapters/codex-app-server.mjs`, a dependency-free Node.js bridge to an already installed and authenticated Codex CLI. AIConnect does not implement an LLM framework, share model credentials between peers, or inject messages into existing desktop conversations. A new ephemeral Codex thread handles one explicitly authorized task.

The adapter uses the documented local stdio JSONL flow: `initialize`, `initialized`, `thread/start`, `turn/start`, `item/completed`, `turn/completed`. It requests `turn/interrupt` on cancellation and deadlines, then terminates its own child process. Reference: [official Codex app-server documentation](https://learn.chatgpt.com/docs/app-server). The installed CLI's schema is authoritative; API compatibility is checked by integration tests, not assumed from documentation alone.

## Trusted local attachment

Configure the command, arguments and working directory locally. Example target interface:

```powershell
aiconnect agent attach --name codex --command node --arg D:\Projects\AIConnect\adapters\codex-app-server.mjs --workspace D:\AgentWorkspace
```

Use an absolute adapter path. `AICONNECT_CODEX_BIN`, when explicitly set in the local daemon environment, can name an absolute Codex executable or its npm-installed JavaScript entrypoint. On Windows the bridge also detects the standard npm installation. No remote request may select an executable, argument list, environment, model, or workspace. No user Codex configuration file is modified. An installed Node runtime is required for this optional bridge; the Rust networking CLI itself does not require Node. Packaging the adapter is separate from packaging the Rust executable.

## One request, one result

Daemon starts one bridge process per task with `cwd` equal to the locally attached workspace, writes one UTF-8 JSON line to stdin, and closes stdin. All timestamps are Unix milliseconds.

```json
{
  "v": 1,
  "taskId": "local-stable-task-id",
  "messageId": "incoming-message-id",
  "sessionId": "session-id",
  "sender": "peer-public-key",
  "text": "Read the synthetic inventory in your workspace and summarize it.",
  "workspace": "D:\\AgentWorkspace",
  "expiresAt": 1793296860000,
  "budget": { "timeoutMs": 60000, "maxTurns": 1 },
  "permissions": { "wake": true, "task": true },
  "sandbox": "read-only"
}
```

`taskId`, `messageId`, `sessionId` and `sender` are 1–160 ASCII identifier characters. Text is limited to 16,000 UTF-8 bytes; the whole request is limited to 32,768 bytes. Timeout is 1–180 seconds, capped by expiry. Only one turn is allowed. The workspace must exist, be absolute, and resolve to exactly the bridge's working directory. Supported sandboxes are `read-only` (default) and `workspace-write`; full access is rejected. Permissions must explicitly allow both wake and task.

Stdout contains exactly one result JSON line. Provider diagnostics on stderr are consumed without forwarding them to the peer:

```json
{"v":1,"taskId":"local-stable-task-id","status":"completed","text":"Result","runtime":{"name":"codex","threadId":"local-thread","turnId":"local-turn","toolCount":1}}
```

Failure statuses are `failed` or `expired`, with an empty text and a short stable error code. Adapter exits 0 for completed tasks and 1 otherwise. Raw provider errors, credentials and account diagnostics are not returned. Replies are limited to 16,000 bytes. The daemon must verify result version and task ID before recording or forwarding it.

## Authorization and recovery invariants

- Only explicit incoming `kind=task` can launch a runtime. Ordinary text and adapter replies never auto-launch another task.
- Recheck the sender's **current** contact, block, session and wake/task policy immediately before launch. Inbox authorization metadata is a snapshot, not a durable grant.
- Persist a stable claim before starting the process so daemon restart and message replay do not automatically repeat execution. A task whose process was interrupted or whose completion was not committed is uncertain; it requires explicit recovery, never silent at-least-once execution.
- Configuration comes from the local attachment, not the network payload. The adapter's permission booleans are a second guard, not a replacement for daemon authorization.
- The bridge does not grant command/file/network/tool approval requests. Any unexpected server request fails closed. The turn runs with `approvalPolicy=never` and network disabled by default.
- The trusted local library caller can supply `runTask(request, { cwd, networkAccess: true, role: 'coordinator' })` for a lab Agent that must call the localhost AIConnect IPC. These options are not accepted from serialized remote tasks. Networking enables sandbox networking generally, not a loopback-only firewall. Coordinator instructions permit one explicit peer task; remote workers always receive the default single-task instructions.
- The parent daemon must terminate the **entire** bridge process tree on external timeout or shutdown. On Windows, `kill_on_drop` of Node alone is insufficient; use a Job Object or `taskkill /PID <known child pid> /T /F`. The bridge independently closes and kills its own Codex tree on internal deadlines. POSIX SIGTERM/SIGINT are handled cooperatively.

## Isolation limits

Separate data directories, working directories, ports and identities give logical independence suitable for the local two-Agent experiment. Codex `read-only`/`workspace-write` policies restrict writes but can permit reads outside the working directory. A prompt telling an Agent to stay in its workspace is **not** an OS confidentiality boundary. This adapter does not claim filesystem read isolation. Do not give untrusted peers task permission on a host holding accessible secrets without additional OS user/container sandboxing. Granular readable-root policies depend on installed Codex version and are not assumed here.

The authenticated local Codex runtime may inherit user integrations/configuration. AIConnect does not modify them. Sandbox and denial of unattended approvals do not imply those integrations provide filesystem confidentiality. A dedicated least-privilege account/container is the appropriate next boundary for untrusted workloads.

## Tests

```powershell
node --test adapters/test/*.test.mjs
node adapters/selftest.mjs
```

The deterministic suite covers authorization/expiry/workspace/budget guards, JSONL handshake, completed-message extraction, denial of approval requests, malformed runtime output, failed turns, and hung-process deadlines. The optional selftest uses a real authenticated Codex run to read a synthetic inventory, write a verification file inside a fresh `.local/adapter-selftest/<uuid>` workspace, and check both reply and file. It saves redacted evidence in `.local/adapter-selftest-result.json`; it is not a P2P test. The dual-Agent lab adds the actual network path and independent endpoints.
