# AIConnect 集成选型调研

日期：2026-10-25。原则：优先集成、最少胶水。三个研究方向并行：通信总线、Agent 运行与唤醒、网络与数据。

> 后续约束更新：P2P 优先是核心硬要求。下文将 Relaycast 作为整套 PoC 首选的旧建议已不再适用于主方案。请优先阅读 [P2P 深入调研](P2P-RESEARCH.md)，其中包含固定版本源码核查和 Windows 本机实测。

## 结论

最省工程的整套候选是 AgentWorkforce/relay + 自托管 Relaycast。但它是中心式消息系统，不能声称完全满足 P2P 和每端本地完整历史。最贴近既定 P2P 目标的整套候选是 ExaDev/agent-comms，但内存状态与唤醒/平台边界需要补齐。没有核实到能同时满足所有条件且可直接商用嵌入的一套成品。

现有代码是无依赖的静态网页原型与静态 Node 服务，尚无真实 Agent、网络或数据库。本轮只新增此调研文档，不改变产品架构，也不安装候选依赖。

## 候选决策表

| 候选 | 能直接承担 | 最重要缺口 | 决策 |
| --- | --- | --- | --- |
| AgentWorkforce/relay + Relaycast | 人与Agent身份、群DM/频道、消息历史、SDK/CLI/MCP、harness启动与投递 | 中心式；engine单机SQLite非每端副本；Windows主要PTY，结构化审批不可假定 | 最少胶水整套PoC首选，接受的架构偏差须明确 |
| ExaDev/agent-comms | 房间、私信、在线状态、跨Agent桥接、直连/relay、设备信任 | 主要状态内存；离线和重启恢复；Windows支持未验证 | 原生P2P对照PoC |
| c2c | 持久收件箱、房间、CLI、多个客户端适配 | Alpha、Linux主测；远程/群消息对managed Codex不保证自动开回合；许可未核定 | 参考唤醒契约，暂不作默认底座 |
| Happy Agent | 守护进程、本地SQLite、REST/SSE、持久会话、任务与审批 | 自有统一harness，不能当成原生CLI透明包装；跨机发现还需网络 | 愿意统一runtime时再选 |
| OpenClaw | Gateway、远程节点、执行审批、hooks、session工具 | 它的节点模型不是任意原生Agent好友网络；要跟随其运行时和安全模型 | 已有OpenClaw用户的可选适配 |
| @number0/iroh | 内嵌Node的加密P2P、打洞、relay、endpoint identity | 不包含消息持久化、群聊和Agent唤醒 | 独立产品网络底座首选备选 |
| Tailscale | 设备身份、发现、网络策略、直连和relay | 单独安装/登录；不提供Agent消息语义 | 用户已有时作为PoC网络 |
| sendme | 文件/目录的P2P传输、校验、断点续传 | 需管理sender生命周期与票据；非完整附件权限系统 | 迁移文件优先复用 |

## Relay 体系的最小集成

AIConnect UI / Skill → 本地薄API → Relay SDK → Relaycast + 每台设备的broker/harness。

不要同时再装另一套消息总线。优先通过现成API映射页面：

| 原型功能 | 复用接口 | 我们补齐 |
| --- | --- | --- |
| 通讯录 | agents/presence | owner、设备、分组的产品映射 |
| 新建/关闭会话 | channels 或 group DM | 默认成员与权限规则 |
| 历史与实时消息 | REST + WebSocket + 现成SDK | 页面状态映射 |
| 唤醒 | harness create / fleet spawn / receiveMessage | 接收设备策略、超时、失败反馈 |
| 取消 | session真实支持的interrupt/release | 不将“停止进程”等同于回滚任务 |
| 共享便签 | 先用固定thread或版本化共享上下文 | 文本视图和写冲突提示；不急着引入CRDT |
| 执行审批 | 运行时真实暴露的审批能力 | 不能用聊天里的“同意”替代系统级授权 |

Relaycast自托管文档给出的启动方式：`npx @relaycast/engine --db ./relaycast.db --port 8787`。这不是我们已安装或验证过的部署。

注意：持久消息不等于任意系统操作exactly-once。engine重启、broker重启、Agent重启是三个不同恢复场景。默认PTY运行路径无法可靠观测所有工具、文件或审批事件，UI须按capability呈现，不应猜测。

## P2P 与本地历史的含义不能偷换

Tailscale把A/B连接起来，不会把中心式Relaycast变成应用层的对等消息网络。engine放在A可实现自托管，但A离线仍影响其他设备访问；它也不等于每个客户端都有完整持久副本。

如果“原生P2P优先+每端本地历史+无需额外VPN”继续作为硬要求，应先验证agent-comms持久化扩展边界；不合适则用官方iroh Node绑定，保留一个Node常驻程序。必须承认后者需要自己实现小型消息层，不能称为零胶水。

## 包与许可证证据

直接向npm查询到：@agent-relay/harnesses 14.0.0、Node>=22、Apache-2.0；@relaycast/engine 11.0.0，依赖better-sqlite3；@number0/iroh 1.2.0、Node>=20.3、MIT OR Apache-2.0，并列有win32-x64-msvc/win32-arm64-msvc原生可选包。发布证据不等于本机运行或双机互通测试通过。

同名项目要区分：AgentWorkforce/relay、bethvourc/agent--relay、joniba/agent-relay彼此不同。后两者分别偏本地handoff、Copilot扩展，不能把能力拼到第一个项目名下。

coder/agentapi当前已归档且README标deprecated，不作为新核心依赖。MCP Agent Mail Rust当前带OpenAI/Anthropic Rider，不能简写普通MIT；c2c许可本轮未核定，不默认可嵌入分发。

## 刻意不引入

Matrix：成熟但homeserver与密钥管理改变目标架构。NATS：增加服务和subject权限映射，不能替代Agent执行。Syncthing：可选目录同步，不能同步活跃SQLite或当消息队列。Yjs：真实并发便签编辑出现后再上。Headscale/tsnet：当前Node项目不为首个PoC增加Go组件与控制面运维。

## 下一步验证门槛

1. 两台Windows分别接一种现有Agent，真实消息触发新工作回合。
2. Agent空闲、忙碌、退出三种状态分别测试；投递成功和唤醒成功分开记录。
3. 单机和双端重启后历史仍在；未完成任务不重复执行。
4. 系统操作审批来自实际runtime，不从终端文本猜测。
5. 原生直连不可用时relay可用；中转不能读取私密内容的保证单独核实。
6. 实测安装依赖与后台进程数量，确认没有通过“集成”转嫁更多配置给用户。

## 官方资料

- https://github.com/AgentWorkforce/relay
- https://github.com/AgentWorkforce/relay/blob/main/docs/self-hosting/README.md
- https://github.com/AgentWorkforce/relaycast/blob/main/docs/self-hosting.md
- https://dev.agentrelay.com/docs/relaycast-api
- https://agentrelay.com/docs/harnesses
- https://github.com/AgentWorkforce/relay/blob/main/packages/harnesses/README.md
- https://github.com/ExaDev/agent-comms
- https://c2c.im/wake-contract/
- https://c2c.im/clients/feature-matrix/
- https://github.com/slopus/happy-agent
- https://github.com/slopus/happy-agent/blob/main/docs/architecture.md
- https://happy.engineering/recepies/rest-api/
- https://docs.openclaw.ai/platforms/windows
- https://github.com/n0-computer/iroh-ffi/tree/main/iroh-js
- https://github.com/n0-computer/sendme
- https://tailscale.com/docs/reference/connection-types
- https://github.com/coder/agentapi
- https://raw.githubusercontent.com/Dicklesworthstone/mcp_agent_mail_rust/main/LICENSE

## 运行层补充：保留原生 Agent 的薄接口

若不接受Happy统一harness或Relay默认PTY的边界，优先评估Agent Client Protocol的现成适配器（codex-acp、claude-agent-acp），以session/new、session/prompt、session/cancel、session/update连接已有Agent。协议恢复能力需协商，不能假定任意运行时都能恢复或接管已打开的GUI会话。它只统一本地运行接口，不负责跨机网络。

- https://github.com/agentclientprotocol/codex-acp
- https://github.com/agentclientprotocol/claude-agent-acp
- https://agentclientprotocol.com/protocol/v1/session-setup

Happy Agent REST示例包括 /v0/tasks、/v0/agents/:agentId/send、abort、question/answer、/v0/events/stream。其peer messaging已支持可达节点间的双向消息，但不替产品做设备发现，且它仍是自己的runtime。

- https://happy.engineering/recepies/peer-messaging/

最终不把“最少胶水”当作改变需求的授权：Relay整套是中心式验证路线；原生P2P/每端历史是独立产品的既定目标，尚未变更。只有Windows双机实证结果才能确定主依赖。
