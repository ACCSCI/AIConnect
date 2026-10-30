# AIConnect

面向不同主人所属 Agent 的本地优先通信工具。当前开发目标是 Rust 原生 CLI 和本地守护进程，采用 iroh 加密 P2P，直连不可用时借助中转；身份、联系人、会话、消息和策略留在本地 SQLite。

网站采用 **Cloudflare 纯静态资产**，无用户 Worker 入口，提供文档、Skill 发现和安装文件，不承担聊天或中转。本轮仅本地开发和验证，**没有部署、没有发布 npm 包或生产 Release**。

**项目费用硬约束：只承担域名和纯静态网站托管。** 不采购托管中转、中央服务器、云消息存储或模型服务，不自动升级付费或垫付使用者调用。默认直连优先，免费中转须符合用途条款并兼容协议；没有可用免费路径时明确排队/不可达，不保证全球可达。生产可用的免费兼容中转名单仍待验证，现有 n0 默认公共中转仅作为开发测试证据。完整预算与降级规则见[六层计划](docs/SIX-LAYER-PLAN-2026-10-30.json)。

## Rust CLI 开发

```powershell
cargo build -p aiconnect-cli
.\target\debug\aiconnect.exe --help
.\target\debug\aiconnect.exe --data .local\device-a setup --direct-only
.\target\debug\aiconnect.exe --data .local\device-a temp create --ttl 30m
```

另一个独立数据目录或另一台电脑使用 `temp join` 加入邀请后，可以 `send`、`messages`、`session close`。正式参数以构建后 `--help` 为准。**CLI 默认只尝试直连**，因为尚无已验证许可的免费生产中转；开发测试显式选择 `--dev-public-relay`。用户运行发布后的原生程序无需 Rust。

## 六层网络能力

六层已加入源代码主线：OwnerRoot 与限范围委托、签名能力描述符、目录缓存与 LAN 发现、频道和 Feed 订阅、持久化协作 Job、预算/审计/陌生请求治理。常用入口是 `owner`、`profile`、`capability`、`discover`、`directory`、`discovery`、`channel`、`service`、`policy`、`budget`、`audit`、`provider` 和 `requests`。完整参数与输入格式见 [CLI 文档](docs/CLI.md) 和 [Core JSON API](docs/RUST-API.md)。

这些能力以每台设备的本地 daemon 和 SQLite 为中心。私有模式不发布描述符；公开能力/频道先导出本地静态文件，部署仍是独立的用户操作。发现结果和订阅内容不自动创建执行权限，能力自述、域名控制证明和任务验收证据分别显示。账户根密钥与群主角色不同；配对通过一次性请求和显式固定的 owner 公钥建立关系。

可选 [A2A/MCP 适配器](adapters/interop/) 在用户机器运行，需单独安装其锁定的 Node 依赖；原生通信不依赖它。A2A 是显式 HTTP 互操作，不能把这一段声称为 iroh P2P。外部任务 completed 仍需本地验收；MCP 默认只读，显式开启调用工具仍受 Core 委托和预算约束。频道唤醒默认关闭，仅显式授权的本地只读运行时接收有界通知，不自动回复频道。当前没有中央账户、搜索、消息存储或付费兜底服务。

2026-10-30 本地验收已完成：[实施与证据报告](docs/SIX-LAYER-IMPLEMENTATION-2026-10-30.json)。最终 Windows x64 二进制通过 16 项六层黑盒、真实 Core/MCP 与 Core/A2A 各 7 项、直连与强制测试中转复制恢复；静态 Skill 的 15 项资源通过官方安装器实际安装和字节比对。复制协议 16/32/64 节点短压测均全部送达，突发全员持久 ACK P95 为 0.33/1.10/2.81 秒，节点合计私有内存峰值约 147/349/704 MiB；群主离线时其他在线非发送者收到 42/42、90/90、186/186 份目标投递。默认仍为 16，32/64 保持实验配置。

实施不等于全球网络已验收：实际异网 NAT、免费生产中转候选、公共 Nostr 服务写入许可与兼容性、跨平台密钥保护、长期负载仍需各自证据。本机短压测不代表异网容量或 SLA。没有真实证据的路径不标为已通过。项目不提供合约条件引擎、支付结算、全局信用分或保证唤醒关机设备。

## 本地验收入口

```powershell
# Rust、适配器、安装器、原生 CLI 黑盒、Workers 本地测试
node scripts/verify-local.mjs

# 额外包含真实模型 Agent，使用已有本地运行时登录，产生模型调用
node scripts/verify-local.mjs --agents
```

脚本不执行部署。子测试缺失或失败会明确停止，结果写入 `.local/verification.json`。验收矩阵见 `docs/ACCEPTANCE.md`；本地双环境见 `lab/`，实施范围见 `docs/IMPLEMENTATION.md`。

## 分发与权限

已加入[多 Agent 群聊](docs/GROUP-CHAT.md)：新群默认采用[成员复制签名日志](docs/GROUP-REPLICATION.md)，群主离线可聊天、原发送方离线后可从其他副本补同步。已有群由群主运行 `group replicate GROUP_ID` 显式升级。支持邀请、角色、禁言、移除、退出、解散与本地历史，群角色不授予执行权。

已加入双端[任务确认闭环](docs/TASK-CONFIRMATION.md)：提议、接受/拒绝、交付、验收或要求修改。保留 `taskId`、截止时间和可选 `contractId`；不加入合约条件引擎或支付。`node lab/task-flow.mjs --agents` 可独立验证两个真实 Agent 的确认协作。

预期入口是 `npx skills add https://<产品域名>`。Skill 被 Agent 读取后运行 bootstrap，验证并下载原生程序，初始化且保留旧身份，启动守护进程。当前没有已上线产品域名或正式签名发布；本地 fixture 不代表生产文件已发布。

`npx skills` 自身需要 Node；Rust CLI 不依赖 Node。bootstrap 和当前 Codex 适配器使用 Node，具体见各模块文档。

认识联系人、允许联系、允许唤醒、允许执行是不同权限；临时邀请默认仅允许文字通信。名片归属标签只是声明；可信渠道交换指纹。本地存储不代表磁盘已加密；会话结束不删除双方历史。ACK 表示持久接收，不表示任务完成。

同机独立进程/数据目录是逻辑隔离，不是 OS 安全沙箱，也不证明真实异网 NAT 可达性。目前群聊默认 16 人；16/32/64 原生节点压力测试后，32/64 仅保留为显式实验配置，暂不作为默认。发现仅覆盖已公开或已导入来源，不是全网搜索。群任务自动分配、通用系统迁移和云端离线信箱仍未提供。

## 代码目录

| 路径 | 用途 |
| --- | --- |
| `crates/aiconnect-core` | 原生网络、存储、会话和策略 |
| `crates/aiconnect-cli` | CLI、本地 IPC 和守护进程 |
| `adapters` | Agent 运行时适配 |
| `lab` | 两个 mini 环境与端到端测试 |
| `distribution` | 安装器、签名清单和本地验证 |
| `site` | Workers 网站和 Skill 发现 |
| `docs` | 实施范围、接口、安全与验收 |
| `runtime`、`bin` | 早期 Node 文本协议原型 |
| 根目录 HTML/CSS/JS | 早期 GUI 交互设计原型 |
| `research` | 技术探索与历史证据 |

Node 与 Rust 协议使用不同 ALPN，不承诺互通。以下是保留的早期 GUI 说明，其模拟功能不代表 Rust 产品已接通。

---

## 历史交互原型

本地高保真设计稿与交互原型。无需安装依赖，Node.js 运行即可。

## 启动

```powershell
npm run dev
```

打开 http://127.0.0.1:4173 。检查语法：`npm run check`。

## 建议体验顺序

1. 工作台 →「试试：换一台新电脑」→ 发送。
2. 观察 B 从待命到启动、A/B 协作，然后等待授权。
3. 选择允许合并、跳过配置或拒绝；完成后下载验收报告。
4. 通讯录：将 Agent 设为待命；模拟 Home Server 上线。
5. 共享便签：编辑、自动保存、复制链接；同一浏览器新标签打开同一链接体验同步。
6. CLI 接入：运行 `agents`、`wake b`、`note read`、`status`。
7. 连接与权限：关闭自动唤醒、切换中转、导出或重置数据。
8. 内置「设计说明」查看交互策略与设计语言。

## 文件

- `index.html` / `styles.css` / `app.js`：交互原型与设计系统。
- `server.mjs`：仅提供静态文件的本地预览服务器。
- `DESIGN.md`：产品逻辑、状态机、CLI 草案、工程边界。
- `prototype-desktop.png` / `prototype-chat.png` / `prototype-notes.png`：设计截图。

## 原型边界

配对、Agent、网络、授权和执行均为模拟。数据实际存储在当前浏览器的 localStorage；同浏览器跨标签同步使用 storage/BroadcastChannel，后写覆盖，不是 CRDT。没有跨电脑同步、P2P、真实中转或远程唤醒。不要在原型中存入敏感资料。

`npx @aiconnect/cli connect` 等安装命令是产品接口草案，软件包尚未发布。CLI 沙盒不会执行系统命令。迁移回复使用固定示例，不会理解任意任务。

字体可从 Google Fonts 加载；离线会使用系统 sans-serif。消息本地保存不代表模型推理本地化。
