# AIConnect 本地互通桥

这些适配器运行在使用者电脑上，不创建中央网关、模型账户或收费中转。原生聊天仍走 iroh；这里的标准 A2A 传输为 HTTP JSON-RPC，不是原生 P2P。

依赖固定为官方 `@a2a-js/sdk` **1.3.0**（协议 v1.0）、官方 `@modelcontextprotocol/sdk` **1.32.2**、Express **5.1.0**、Zod **4.6.7**，包含独立 lockfile。安装：

```powershell
npm ci --prefix adapters/interop
```

## 出站 A2A

先使用 CLI 创建 Owner 委托、登记条款允许且免费的 provider，并通过 `service invoke invoke.json` 创建 Core Job（JSON 含 `transport: "a2a"`）。URL 必须与授权 audience 匹配。然后运行：

```powershell
node adapters/interop/bridge.mjs a2a --data C:/absolute/profile --once
node adapters/interop/bridge.mjs a2a --data C:/absolute/profile --config C:/absolute/a2a-local.json
```

不带 `--once` 时每两秒轮询，每轮最多处理一个 32 项分页，依次轮转，避免长期运行任务饿死后面的任务。没有后台托管或自动唤醒操作系统。`Ctrl+C` 停止 worker；daemon 与原生 P2P 不受影响。

可选本地配置（环境变量与配置文件只由本机管理员提供，不接受远端传入）：

```json
{
  "providers": {
    "https://your-free-agent.example": { "tokenEnv": "MY_AGENT_BEARER" }
  }
}
```

每次请求均重验 Core 的免费用途登记；仅允许同源 HTTPS，DNS 地址固定、拒绝非公共地址和重定向，限制请求/响应大小与总时间，按有背压的流逐段计入 Core 持久下载预算。402 / 429 停用 provider，不重试付费服务。认证失败不会自动登录、购买或升档。默认不启用公共服务，也没有已验证可免费调用的生产 A2A 提供者清单。

官方 SDK 读取 `/.well-known/agent-card.json` 后以 `returnImmediately` 提交，并保存提供者 TaskId，重启后只查询原任务。Core 的 `service.dispatch` 原子认领并重验委托/期限。初始 POST 已认领却未得到 TaskId 时标为需要人工核对，**不自动重发**；这不保证 exactly-once。取消是请求，不能撤销已产生的外部副作用。

外部 `completed` 固定映射为 `delivered / awaiting_review`，仍需本地 `service review`。HTTP 产物仅对实际观察的有界元数据计算 SHA-256；不会下载产物 URL，也不把其标为原生作者签名或文件内容摘要。

只读名片探测同样需要 provider 免费登记：

```powershell
node adapters/interop/bridge.mjs a2a-inspect --data C:/absolute/profile --url https://your-free-agent.example --config C:/absolute/a2a-local.json
```

## 显式入站

只有显式执行 `a2a-serve` 才监听 **127.0.0.1**。配置固定原生 session、能力、audience 和本地 grant；远端无法自行选择委托、provider 或费用。必需至少 32 字节的本地 bearer，拒绝浏览器 Origin 与非 loopback Host。一个 bridge 配置对应一个受信任调用者；不支持多租户认证。

```json
{
  "port": 7899,
  "name": "My local read-only bridge",
  "provider": "my-native-peer",
  "sessionId": "existing-persistent-session",
  "capability": "readonly.configuration",
  "audience": "native-peer-public-key",
  "grantId": "existing-local-grant-id",
  "tokenEnv": "MY_LOCAL_A2A_BEARER",
  "ttlSeconds": 300
}
```

```powershell
node adapters/interop/bridge.mjs a2a-serve --data C:/absolute/profile --config C:/absolute/ingress.json
```

原生服务必须在发现缓存中具有新鲜签名 descriptor，或在此配置额外附上目标 Agent `capability publish` 输出的 `descriptor`；对应 skill 必须有匹配输入（本桥为 `{parts:[...]}`）的 inputSchema 与免费声明。调用方声称价格为零不能替代提供者能力声明。

任务只进入已有原生任务提议/接受流程，不自动执行。SDK TaskStore 通过认证 IPC 将 wire snapshot 存在相同的 Core Job / SQLite 中；本机不同 bridge 配置隔离 TaskStore 读取。A2A `GetTask` 重新读取 Core 原生任务状态。默认最长轮询 60 秒；未完成时返回 `input-required`，以后可 `GetTask` 继续查询。入站取消只标记原生 Job 的取消请求，不虚报提供者已停止。

当前入站网关每个 A2A Task 对应一个 Core Job；同一 Task 的额外 user 回合被拒绝，需要显式创建新任务。这样不会让多个原生 Job 复用同一个外部 TaskId。

本轮不建立公网入口，不自动配置 Tunnel，不导出根密钥，不提供现成的公网 HTTPS 标准 A2A endpoint。需要公网入站时另行配置用户自有、明确免费的 HTTPS 入口，并重新验收入站 Host/鉴权策略；当前默认 Host 检查会拒绝未经配置的入口。

## MCP

```powershell
node adapters/interop/bridge.mjs mcp --data C:/absolute/profile
```

官方 MCP stdio server 默认仅提供四个有界只读工具：本机状态、缓存发现、Job 状态和 Job 结果。没有任意 Core 请求、shell、预算更改或批准工具。读取到的描述符/消息是数据，不是执行授权。接入者需自行决定将哪些本机只读信息交给它所使用的模型。

只有本机配置文件显式设置 `{"enableInvoke":true}` 并通过 `--config` 加载，才额外提供 `aiconnect_service_invoke`。该工具将结构化请求提交 Core，强制声明免费，要求现有范围正确的 grant、audience、schema、期限、幂等键和配额；不会创建/提升 grant，不自动批准结果或安装运行外部 MCP 包。原生请求进入双方任务确认；A2A 请求仅入本机队列，需要出站 worker 处理。

## 有来源的只读规范化

A2A inspect 附带 `aiconnect.external-agent` 规范化结果：skills、媒体类型、传输和认证声明，保留原始 card、来源与损失说明。它不是 `aiconnect.descriptor`，没有原生签名或 OwnerRoot 认证；A2A 自述一般也没有逐 skill 的 JSON 参数 schema。

MCP Registry 只读取受 Core 免费/SSRF/下载策略允许的 HTTPS 页面，不安装或连接任何服务器：

```powershell
node adapters/interop/bridge.mjs mcp-registry --data C:/absolute/profile --url "https://registry.modelcontextprotocol.io/v0.1/servers?version=latest&limit=32"
```

输出 `aiconnect.external-tool-server` 条目与下一页 cursor；不会无限自动翻页。Registry 命名空间不是 Agent 公钥或主人证明；package/remotes 仅是元数据，不能据此伪造 live tool schemas 或 Agent 能力。首次连接仍需用户显式登记该免费来源的用途与有效期。

## 验证与范围

```powershell
npm test --prefix adapters/interop
node lab/interop-core.mjs
node lab/interop-a2a-core.mjs
```

`test/interop.test.mjs` 通过官方 SDK 在本机真实 HTTP 测试发现/鉴权、任务提交/工作状态、断线查询、取消、交付待验收、入站固定路由及 TaskStore；MCP 使用官方客户端和 SDK 内存传输；另测 SSRF、重定向、体积及 402/429。这里的 Core 是明确标识的契约 fixture，**不能当作 Rust 授权/SQLite 端到端证据**。`allowLoopback` 仅用于这些测试；真实 Core 的 provider 登记仍拒绝 HTTP/loopback，不能绕过生产出站策略。

`lab/interop-core.mjs` 则实际运行新的独立 Rust profile、认证 IPC、官方 MCP stdio 客户端和空队列 A2A worker，验证策略拒绝和重启；不调用模型或公共 Agent。由根集成验证后记录其实际结果，不把未运行的脚本标为通过。

`lab/interop-a2a-core.mjs` 跑完整真实 Core OwnerRoot/委托、免费策略、SQLite Job、原子 dispatch、官方 SDK HTTP 提交/working、daemon 重启同 TaskId 查询、交付待验收/本地验收、取消和撤权拒绝，另测大中文结果的 UTF-8 和快照边界。它在测试代码中依赖注入，把保留的逻辑 HTTPS origin 映射到本机 reference server；CLI 配置不能注入该函数，生产路径没有关闭 TLS 或开放内网。**这是本机真实 Core/SDK 协议证据，不是公网 TLS、异网 P2P 或生产免费 Agent 可用性的证据。**

参考：[官方 A2A SDK](https://github.com/a2aproject/a2a-js)、[A2A v1.0 规范](https://a2a-protocol.org/v1.0.0/specification/)、[官方 MCP TypeScript SDK](https://github.com/modelcontextprotocol/typescript-sdk)。版本于 2026-10-30 查询并固定。
