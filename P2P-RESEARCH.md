# AIConnect：P2P 核心要求深入调研

日期：2026-10-25。三路源码研究 + Windows 本机实证。结论替代 INTEGRATIONS.md 中的中心消息服务优先建议。

> 后续实测更新：已完成持久消息/接收进程崩溃/去重、真实 Codex 启动与取消，以及本机双进程 P2P→落盘→真实 Agent→P2P 回传；Pear 加密多写离线追赶也已测试。详见 [实证测试报告](TESTING-2026-10-25.md)。以下“待测”描述以该报告的新状态为准，跨 NAT 和持续强制中转仍未测。

## 1. 决策结论

主候选：官方 `@number0/iroh` + Node 本地常驻程序 + SQLite 小型消息信封层 + 现成 Agent 运行接口。备选：Hyperswarm / Corestore / Autobase 的 Pear 官方聊天骨架。

不把中心式 Relaycast、Matrix、云消息服务加 VPN 作为主线。应用正文在设备之间传输，中转是连接兜底，不是必须经过的消息主库。设备自己保存历史。

“最少胶水”指少维护网络协议、同步算法和运行时适配，而不是把持久消息与可验证执行偷偷省掉。当前没有核实到同时满足全部要求、可直接套壳的 Agent IM 成品。

## 2. 研究证据范围

| 路线 | 固定版本 | 证据 |
| --- | --- | --- |
| iroh Node | npm 1.2.0；源码tag commit 66a8d619a4d791d458d582f312df6e0ab42bc541 | 发行包类型/源码、CI、本机双进程测试 |
| Agent Comms | 8.15.18；commit 1a0e61a9ae5eb80c4cd55eb010e9fa77051e47d8 | 实际路由、TLS transport、内存store、hooks源码 |
| DecentNetwork peer | npm 0.1.168；gitHead 3a8b9af7f6832e3672e52736cee6e0008046db33 | 发布包发送策略、ACK、许可源码 |
| Hyperswarm / Corestore | 4.17.2 / 7.16.1 | 发布包、官方文档、Windows本机测试 |
| Pear chat scaffold | published commit 0b07ffb5e8fbaedfe59f59b67138149329f86739 | Corestore/Hyperswarm/Autobase/HyperDB实现源码 |

源码和测试均保存在 research/。隔离安装使用 --ignore-scripts；主产品 package.json 与服务没有变化。不执行下载仓库的安装指令、部署脚本或Agent指令。

## 3. 被深入核查纠正的旧判断

### Agent Comms

有房间、私信、群fan-out、设备信任和中转端到端加密，但其实际路由是“已存在peer session则直发，否则hub”。TLS/TCP host:port连接并不是公网NAT打洞。收到hub目录仅合并和触发重试，未发现完整主动探测和直连升级路径。

默认房间、消息、DM和重试队列仍在内存。新增durable notice为本地签名日志，尚未接通announce/fetch/catch-up。Windows路径还存在which、bash、ps等依赖。直接套它需要补核心机制，已不再是少量边缘胶水。

详见 research/agent-comms-readonly/RESEARCH.md。

### Beagle / DecentNetwork

MIT skill不等于底层SDK的许可：实际@decentnetwork/peer是GPL-3.0-or-later。聊天和控制包即使直连新鲜也会再发relay副本，不能称为直连成功时不经中转。其ACK在业务onText前，去重ID延迟落盘且不是保存正文，不能当作我们的收件箱已经提交。

因此不把它作为默认嵌入依赖。许可事实不等于具体产品法律结论；这里只据此标记发行与维护成本。

详见 research/p2p-alternatives/AUDIT.md。

## 4. iroh 本机实测

环境：Windows win32 x64，Node v24.22.0；安装@number0/iroh@1.2.0及win32原生包，共两个安装包，无本地Rust编译。

### A. 完全禁用relay/discovery的双进程收发

- 启动独立发送/接收Node进程。
- 交换含端点公钥和地址的ticket。
- 验证远端身份；双向QUIC stream传UTF-8 JSON消息和应用ACK。
- 路径显示isSelected=true、isIp=true、isRelay=false。
- 相同序列化私钥重新绑定后公钥一致。
- 路径监听与stop、endpoint.close正常。

这是同一Windows主机的UDP直连，不是两台设备或跨NAT测试。密钥恢复是同一测试中重新绑定序列化密钥，不是已完成生产密钥保管系统。

### B. public relay bootstrap再升级

- receiver只共享relay地址ticket，不附带直连地址；禁用额外地址发现provider。
- 观察连接初始选中isRelay=true。
- 之后观察isIp=true且isSelected=true，中转路径不再选中。
- 中文消息、应用ACK和身份校验正常。

此测试访问了开发公共relay，传输随机身份和合成消息；未发送产品数据。它证明本机当前网络可使用relay并升级直连，不能证明UDP被封锁后的持续中转、不同运营商NAT成功率或生产SLA。

### 发现的实际接口缺陷

watchPathEvents的发行d.ts宣称单参数event，但实测回调是(error,event)，按声明写会在event=null时崩溃。适配为error-first后通过。应锁版并隔离在IrohTransport内；参考 research/iroh-smoke/CALLBACK-ISSUE.md。

可复现：

```text
cd research/iroh-smoke
node smoke-direct.mjs
node smoke-direct.mjs --relay
```

结果：result-direct.json、result-relay-bootstrap.json。结果已去掉本机IP地址。

## 5. P2P优先需要区分的三个概念

1. 数据拓扑：A和B是最终加密端点，能直连就不靠消息主服务器转发。
2. 建连引导：relay可以辅助发现和打洞；iroh默认可能先经relay建连再升级，不能承诺首包从不经过relay。
3. 正文策略：可先给直连短暂探测预算，看到选中IP路径就发送；到期才允许加密正文走relay。网络变化后继续动态选路，不能凭一次路径快照证明后续每个包的路径。

建议产品默认采用“正文直传优先、加密中转兜底”，显示真实路径及变化。不要把 relay online 当作对端在线，也不要把‘已写网络’当作已落盘、已读取或已执行。

公共relay仅用于开发测试；生产需要自己的或有服务约定的relay。relay只转发密文，但仍可观察连接和流量元数据；不是匿名网络，也不自动保存离线消息。

## 6. Pear/Hyperswarm路线

官方已有加密多写聊天骨架：Corestore本地日志、Hyperswarm发现/打洞、Autobase多写复制、HyperDB消息视图、blind-pairing邀请。相比纯iroh，它能省下更多聊天历史复制逻辑。

Hyperswarm已存在打洞失败转forceRelaying的实现，但默认relayThrough为空，需配置blind relay；基础chat scaffold没有自动完成这一步。blind peer是加密离线副本，和网络转发relay不是同一个功能。

Autobase投影视图可能重放/重排，不能在apply回调执行真实任务。消息复制与任务消费必须分开，真实副作用按task ID和步骤ID持久去重。

Windows隔离冒烟已验证Hyperswarm/Corestore可加载，本地直连消息往返和Corestore关闭重开数据恢复。另用独立write/read进程验证了历史跨进程恢复。网络实验的两个peer处于同一Node进程，未测试双进程网络隔离。完整Autobase聊天骨架、不同网络打洞、relay与blind-peer仍待实测。详见 research/p2p-alternatives/ 中脚本和报告。

这条路线适合优先做通用多写IM和离线历史复制；依赖和数据模型较多。不能因组件数量多就直接判为胶水更多，也不能因官方chat示例存在就视为已完成Agent产品。

## 7. 推荐的最小产品结构

```text
网页UI / CLI / Skill
       │ localhost API
每台设备一个Node daemon
  ├─ iroh endpoint：身份、打洞、加密、路径切换
  ├─ SQLite：会话、inbox/outbox、去重、任务状态
  └─ 现成Agent运行接口：创建/恢复/启动回合/取消/事件
```

小群由发件人对成员逐一发送，最初不引入全网DHT通讯录、中心消息主库或复杂群广播。共享便签先用带版本的读写和冲突提示，通过同一P2P通道同步，不再建另一套WebRTC网络。

消息可靠性：发送前outbox事务落盘 → 接收方核对对端身份和会话权限 → inbox/dedup事务提交 → 回ACK → 发件人更新确认。断线重放同一message ID。保证持久接受和幂等消费，不承诺任意系统副作用exactly-once。

Agent消息是数据，不直接继承人类权限。运行时适配优先支持受管理的明确会话，不依赖模拟键盘接管任意已开窗口。端点公钥不自动证明人类归属或执行授权，仍需配对绑定和接收方策略。

双方不同时在线时，消息保存在发件人本地，等待再次联机。若要求发件人也离线后继续投递，必须额外有在线副本/加密信箱；不是把relay改个名字就能得到。

附件优先评估sendme，但它独立进程和临时endpoint不会自动复用JS连接。群成员移除需要新消息密钥策略；撤销访问不等于能删除对方已保存的历史。

## 8. 尚未完成的准入测试

- 两台真实Windows、不同NAT/运营商、真实IPv4/IPv6环境。
- UDP不可达时保持relay可用；恢复UDP后升级直连。
- 两端进程/设备重启后历史与outbox恢复。
- 收件已落盘但ACK丢失、重复投递不重复启动任务。
- 对端空闲/忙碌/Agent退出时的新回合唤醒和取消。
- 非好友、过期授权、被移出群成员、消息大小限制。
- 生产relay部署、带宽和日志边界。

本机测试不可外推公网打洞成功率。现阶段推荐锁定iroh作首个工程验证，保留Pear聊天骨架作可靠历史复制的对照，而不是混合两套P2P网络。

## 9. 官方资料

- https://docs.rs/iroh/1.4.1/iroh/#connection-establishment
- https://github.com/n0-computer/iroh-ffi/tree/66a8d619a4d791d458d582f312df6e0ab42bc541/iroh-js
- https://github.com/n0-computer/docs.iroh.computer/blob/main/add-a-relay.mdx
- https://github.com/ExaDev/agent-comms/blob/1a0e61a9ae5eb80c4cd55eb010e9fa77051e47d8/src/core/wire-mesh-transport.ts#L778
- https://docs.pears.com/p2p/explanation/autobase-backed-chat-room/
- https://github.com/holepunchto/hyperswarm
- https://github.com/holepunchto/hyperdht
- https://github.com/holepunchto/autobase
- https://github.com/n0-computer/sendme
