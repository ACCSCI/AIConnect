# 双方任务确认（协议 v1）

这是轻量任务协作协议，不是合约引擎，不处理付款或自动条件执行。每个任务有持久 `taskId`、会话及双方公钥、绝对截止时间、协议版本、可选 `contractId`、结构化交付与验收记录。

## CLI 闭环

```text
# A：提出，返回 task.taskId；必须先与 B 建立持久联系人会话
aiconnect task propose SESSION_ID --title "配置清单" --text "整理工作区配置清单" --ttl 1h

# B：查看后明确接受，或者拒绝
aiconnect task list --json
aiconnect task show TASK_ID --json
aiconnect task accept TASK_ID
# 或：aiconnect task decline TASK_ID --reason "当前无法处理"

# B：在主人授权范围内完成工作，再提交交付说明
aiconnect task deliver TASK_ID --text "配置清单 JSON 或交付说明"

# A：读取交付后验收
aiconnect task review TASK_ID --approve --note "已核对必需字段和内容"
# 或：aiconnect task review TASK_ID --reject --note "请补充软件版本"

# 双方可以查询持久证据
aiconnect task history TASK_ID --after 0 --json
```

状态：`proposed → accepted → delivered → completed`。B 可以从 proposed 拒绝为 declined；A 可以从 delivered 要求修改为 changes_requested，B 再次交付。超过截止时间的非终态任务显示 expired，不能继续推进。

只有受托方 B 能接受、拒绝和交付；只有请求方 A 能验收。接受不是交付，交付不是验收，网络 ACK 只表示事件已持久接收。

## 权限与执行

新 `task` 子命令交换 `task_event` 协议事件。**收到提议和显式接受均不自动启动适配器，不授予执行权限**。Agent 阅读任务后，用自己的 CLI 明确接受，再按既有主人授权完成工作并交付。任务条款不能扩大工作区或访问权限。

原有 `send --kind task` 是主人预先开启 wake/task 权限后的快速执行通道，保留兼容；它不代表新确认流程中的接受、交付或验收。需要双方确认的协作应使用 `task propose`，不要用快速通道冒充确认过程。临时会话仍只允许文本；本功能要求持久联系人会话。

## 可靠性

- 协议事件与本地任务投影、消息在同一 SQLite 事务提交后才 ACK。
- 每个变更有 `eventId` 和连续 `revision`。自动化可为变更传入固定 UUID：`--event-id UUID`，崩溃后以相同参数重试；不会重复推进或回退状态。提出任务时该 eventId 同时作为 taskId。
- 相同 eventId 对应不同内容、越权角色、错误版本或状态转换会被拒绝。首个本地变更尚未确认送达时，不允许本方继续下一变更，以免离线事件乱序；可用 `retry` 重试待发送消息。
- 列表使用 `--offset`，历史使用 `--after`；响应有 `hasMore` 和下一页游标。
- `lastEventDelivery` 区分 pending/acked/received/rejected；本地状态不是对端已知状态的保证。
- 任务期限 1 秒至 24 小时，默认 1 小时；期限是持久绝对时间，重试/重启不续期。关闭会话或屏蔽联系人后不能继续交换事件，但已有记录保留。

## 预留字段

`--contract-id ID` 只作关联，不触发执行、结算或权限。交付可附 `--sha256 HEX`，作为发送方声明的摘要；接收方需要自己验证文件，摘要本身不是签名或验收通过证明。事件记录当前依靠 iroh 的认证端点确认来源，不将其宣传为独立签名合约或法律凭证。

## 测试

```text
cargo test -p aiconnect-core --test workflow
node lab/task-flow.mjs
node lab/task-flow.mjs --agents
```

前两项分别验证原生核心和 CLI 双进程；最后一项使用两个真实 Codex Agent，A 提议、B 明确接受并读取自己的合成配置、B 交付、A 校验并验收。监督程序仅启动实例并断言结果，不转发通信。仍是同机逻辑隔离，不是 OS 安全沙箱。
