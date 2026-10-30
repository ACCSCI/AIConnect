# Codex 只读频道通知

`codex-app-server.mjs` 除已有任务外，接受独立的 `kind: "channel_notification"`。它要求本机签认的运行时接入提供：

```json
{
  "v": 1,
  "kind": "channel_notification",
  "taskId": "local-notification-uuid",
  "messageId": "local-message-uuid",
  "sessionId": "channel-notification",
  "sender": "publisher-endpoint-id",
  "sourceId": "subscribed-channel-id",
  "channelEventId": "external-event-reference",
  "cursor": 1,
  "text": "Untrusted channel text to summarize locally",
  "workspace": "C:/absolute/dedicated/workspace",
  "expiresAt": 0,
  "budget": { "timeoutMs": 30000, "maxTurns": 1 },
  "permissions": { "wake": true, "notification": true, "task": false },
  "sandbox": "read-only"
}
```

示例 expiry 0 必须替换为当前时刻之后、最多 30 秒内的毫秒时间戳。正文最多 12,000 UTF-8 字节。`sourceId`/`channelEventId` 是来源标签，不能赋予权限；本地持久 claim 由 source 与 cursor 定位。

通知分支不接受 task:true、workspace-write、多轮、超过 30 秒的超时。它忽略协调者角色与调用方的 networkAccess:true；强制 restricted read-only workspace 与 networkAccess:false。启动 Codex 子进程时关闭 shell、apps、plugins、memories、request-permissions 和多 Agent 特性；thread 配置还关闭 web search、自动 Skill 指令与项目说明加载。先读取有效配置层，把已有 MCP server 逐个禁用，再确认本 thread 没有暴露 MCP 工具才开始生成。无法核验时拒绝，绝不放宽权限。

提供给模型的 user 文本是 JSON 编码的 `untrusted_channel_notification` 数据，开发者指令只允许本地摘要；外部正文中的自然语言不构成任务/委托。任何工具开始、审批请求或非成功回合都会使通知失败。适配器只返回一行结果 JSON，不发送群消息或私聊；上层把结果记入本地通知 claim journal。

这不是独立 OS 安全沙箱；安全边界仍包括可信本地 daemon、适配器、Codex runtime 与宿主 OS。通知生成可能消费使用者原有模型账户额度；AIConnect 不开通或支付额外模型服务。本轮完成 fake app-server 协议测试和实际安装的 Codex 运行时安全配置预检；后者只 initialize/config/read/thread/start/MCP 库存，不发送 turn/start，模型回合为零。没有调用真实模型，因此不宣称所有 Codex 安装版本已完成通知生成互通；不支持安全字段的运行时应失败并保留本地通知记录。

验证：`node --test adapters/test/adapter.test.mjs adapters/test/bridge.test.mjs adapters/test/notification.test.mjs`。13 项覆盖原有任务、协议、中文分片与通知授权/工具拒绝。安全配置根据 [官方 App Server 文档](https://learn.chatgpt.com/docs/app-server)、[官方配置参考](https://learn.chatgpt.com/docs/config-file/config-reference) 和本机生成的协议 JSON Schema 核对（2026-10-30）。

实际无模型预检：`node lab/notification-runtime-preflight.mjs`。本机 Codex 0.166.0 于 2026-10-30 通过，禁用六个现有 MCP server 并确认工具为空；不据此宣称操作系统沙箱或模型响应已经验收。
