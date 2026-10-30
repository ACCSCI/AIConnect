# 免费基础设施核验（2026-10-30）

项目只支付域名和静态托管。本次只核验公开资料，没有注册账户、购买、部署、公开发布或调用模型。机器仍只有当前一台，不能据此声称异网/NAT 验收完成。

| 选项 | 核验结论 | 默认处理 |
| --- | --- | --- |
| n0 公共 iroh relay | 免费、限流、无在线保证；官方文档定位开发测试并建议生产不用公共 relay | 仅显式开发测试模式；不列为已批准的生产保障 |
| 第三方志愿 iroh relay | 尚未找到同时有明确免费生产用途许可和当前协议兼容证据的端点 | 生产名单为空，不能编造公共地址 |
| Cloudflare 静态资源 | 适合网站、Skill、主动公开的目录/频道文件；动态 Worker 调用单独计费 | 仅静态内容，不能当私人聊天中转 |
| Nostr relay | 公告网络，独立协议/密钥；免费资格、用途、限额须逐个检查 | 可选公开元数据，不能替代 iroh relay |

运行时应默认尽力直连，没有获准免费兼容中转时保留本地待发并报告不可达。用户可以显式选择开发测试公共中转，或自行提供经过用途与兼容性核验的免费节点。未知价格、HTTP 402/429、收费变化和额度耗尽只会暂停/停用，不自动购买。直连模式可能降低跨 NAT 成功率；这是真实限制，不应用付费回退掩盖。

官方资料：[iroh 中转说明](https://docs.iroh.computer/add-a-relay)、[iroh 定价](https://www.iroh.computer/pricing)、[公共中转与限流说明](https://www.iroh.computer/blog/shared-relays)、[Cloudflare 静态资源计费](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)、[Nostr relay 信息规范](https://github.com/nostr-protocol/nips/blob/master/11.md)。免费、开源和生产使用许可是分别核验的事项，源码可自托管不代表项目获赠一台免费服务器。

结构化证据见同名 JSON。六层本机验收由 `lab/six-layer-acceptance.mjs` 单独记录；资料审查结果不会冒充传输实测结果。

另用 `scripts/six-layer-relay-probe.mjs` 完成两个真实公网 Nostr 只读探针：`nos.lol` 返回 NIP-11 HTTP 200，WSS 订阅返回 EOSE；`relay.damus.io` 返回 NIP-11 HTTP 200 并建立 WSS，但 10 秒内没有 EOSE。两者都没声明 `payment_required: false`，因此免费资格仍是“未声明”，不自动列入批准名单。nos.lol 提供的条款链接未能用研究工具读取，不能推定允许本项目的生产公告用途。

此次没有发送 `EVENT`、没有发布本地描述符或私人数据。原始结果在 `.local/six-layer-relay-probe-1793360892168.json`。探针只证明当前网络中的读取结果；不证明写入往返、存储期限或生产服务承诺。

最终 guard 版 Release 本机黑盒验收 **16/16 通过**，包含默认不启用未经批准的中转、未登记 custom relay 拒启动、direct-only/custom-relay 矛盾配置拒启动、模拟 402/429/收费变化停用、真实 P2P 只读探针、陌生请求不唤醒运行时、同 owner 发现及撤权、频道权限与 Job 本地验收。二进制 SHA256：`927f51e9569c02f1037cbf70710111ebe59460f54ac1dc60ce154f0a1b46514c`。原始报告：`.local/six-layer-acceptance/2026-10-30T12-23-19-027Z-3b1ead/report.json`。所有测试 daemon 已停止。前一版 `0e19d24b…` 的有效历史结果另保存在 JSON 的 preGuardHistoricalReport 字段，不能与本次产物混用。

费用变化用本地 `provider.check` 注入验证；这不表示软件可以自动获知服务商以后修改的商业条款。真实异网/NAT、获准免费生产中转与其他操作系统仍未验收。
