# 多中继调度

本地开发功能，尚未发布。保留官方客户端，由单个 `hbbs` 为新连接选择 `hbbr`。各中继运行带监控接口的本 fork 版本，Web/API 无需修改。

## 分配规则

1. 根据双方在 `hbbs` 上的连接地址查询 GeoIP，计算经过各中继的距离。总距离附加最长一段距离的 25%，避免跨地区连接只照顾一端。
2. 排除故障、监控过期、排空或达到容量的节点。
3. 在距离最优值加 `candidate_slack_km` 范围内，按连接数和带宽占用择优；距离差作为次要因素。附近节点全部满载时，允许使用更远的健康节点。
4. 为分配保留短暂名额，减轻采样间隔内大量连接挤入同一节点的问题。名额在 `poll_seconds * 2 + 2` 秒后过期；它不是会话数的硬限制。

位置未知时按已知一端的位置选择；双方未知时按负载分配。支持 IPv4、IPv6、IPv4-mapped IPv6，以及 `location_overrides` 的最长 CIDR 前缀匹配。

地理位置不等于网络延迟。此版本不测量两端到中继的实际 RTT，也不保证每次选中最低延迟线路。VPN、代理出口、GeoIP 精度和运营商路由都会影响结果。

## hbbr 监控接口

示例为私有网络地址，请替换为节点的实际地址。为每个节点生成独立随机令牌，例如 `openssl rand -hex 32`，通过本地环境文件注入；不要写入仓库。

```dotenv
RELAY_NODE_ID=tokyo
RELAY_METRICS_BIND=10.10.0.2:21120
RELAY_METRICS_TOKEN=<至少32位的随机令牌>
```

启动中继时使用现有 `hbbs` 的公钥：

```sh
hbbr -p 21117 -k '<现有 id_ed25519.pub 的内容>'
```

客户端继续使用同一公钥。无需把 `hbbs` 私钥分发给中继。

`GET /metrics` 需要 `Authorization: Bearer <令牌>`，返回节点 ID、进程启动 ID、运行时间、活动中继会话数和累计转发字节数。每对已配对连接计为一个会话；带宽按双向有效载荷累计一次转发出口计算，不重复累计接收流量，不含 TCP/IP 开销及同机其他服务流量。

未设置 `RELAY_METRICS_BIND` 时不开放接口。接口仅供 `hbbs` 使用，应通过防火墙限制来源。建议走 WireGuard 等私网；公网需用 HTTPS 反向代理到本机接口，禁止缓存。HTTP 监控地址只接受私有或回环 IP，不经过环境代理，不跟随重定向。中继传输端口直接连接节点，不经过普通 Web CDN。

## hbbs 配置

复制 [配置示例](examples/relay-scheduler.json)，修改节点地址、位置、容量及监控地址。

```dotenv
RELAY_SCHEDULER_CONFIG=/root/relay-scheduler.json
RELAY_TOKEN_TOKYO=<与东京节点一致的监控令牌>
RELAY_TOKEN_FRANKFURT=<与法兰克福节点一致的监控令牌>
```

`geoip_database` 指向自行准备的 MaxMind City MMDB，相对路径以配置文件目录为基准。数据库不随本项目分发。更新数据库或节点配置后重启 `hbbs`。

可通过 `location_overrides` 补充或覆盖地址位置，例如测试网络：

```json
"location_overrides": [
  { "cidr": "192.0.2.0/24", "location": { "latitude": 35.68, "longitude": 139.69 } },
  { "cidr": "2001:db8::/32", "location": { "latitude": 50.11, "longitude": 8.68 } }
]
```

没有 MMDB 时可删除 `geoip_database` 字段。未被覆盖规则匹配的地址会按未知位置处理，不会自动查询外部定位服务。

| 参数 | 含义 |
| --- | --- |
| `address` | 两端客户端可访问的中继地址，必须写明端口；IPv6 使用 `[地址]:21117` |
| `metrics_url` | `hbbs` 可访问的监控地址；与客户端中继地址可以不同 |
| `max_sessions` | 调度会话容量，应按实际负载测试设置 |
| `bandwidth_mbps` | 调度带宽容量，单位为十进制 Mbps；建议给系统及其他服务预留余量 |
| `drain` | 停止新分配；已经发出的分配仍可在节点健康时完成握手 |
| `candidate_slack_km` | 地理候选距离余量，越大越容易跨区域分担负载 |
| `poll_seconds` | 每轮并发检测完成后等待的秒数，默认 3 秒 |
| `timeout_ms` | 每节点 TCP 检查和监控请求的整体超时，默认 1500 毫秒 |
| `stale_seconds` | 状态最长有效期，默认 12 秒 |
| `recovery_successes` | 启动及故障恢复所需连续成功采样次数，默认 2 次 |

开启后取代 `-r` 的轮询列表。不能与 `--mask` / LAN 中继地址覆盖同时使用。客户端的“中继服务器”留空，让 ID 服务分配；ID 服务器、API 和公钥保持原有配置。

## 故障与维护

- 同时检查中继 TCP 端口和带认证的监控接口。一次检测失败即停止新分配；默认两次连续成功后重新加入。
- 正常情况下故障会在约一轮检测时间内被发现；检测停止时，过期状态同样不可选。公网 TCP 可达不代表每位用户的线路都可达。
- 节点启动或重启后须重新取得两次监控样本，避免把未知负载当作空闲。
- 所有节点不可用时不退回已知故障节点。允许直连时仍可尝试 P2P；`ALWAYS_USE_RELAY=Y` 时返回暂无可用中继提示。
- 分配后到握手完成前发生故障，会提示重新连接。官方客户端保留最初中继地址，服务端不能在握手中途单方面改地址。
- 已有连接不会因负载变化主动迁移；承载该连接的中继宕机仍会断线，需要重新连接。
- 修改 `drain` 并重启 `hbbs` 可排空节点，不必重启 `hbbr`。取消 `RELAY_SCHEDULER_CONFIG` 并恢复 `-r` 可退回原轮询模式。

通过本机 `hbbs` 管理端口（默认 `127.0.0.1:21115`）执行 `rst` / `relay-status` 查看节点健康、会话数及带宽。`tg <IP1> <IP2>` 可预览分配，不占用名额。不在公网开放该管理端口。

## 本地构建和验证

Windows：`powershell -File scripts/build-relay-local.ps1`。

Linux：`sh scripts/build-relay-local.sh`。

产物在 `artifacts/relay/`，镜像为 `rustdesk-server:relay-local`，包含 `hbbs`、`hbbr`。原有发布镜像标签不变。

验证覆盖双方位置、IPv6、未知位置、满载/带宽/排空、状态过期与恢复、监控认证、真实 TCP 中继转发、并发配对、节点宕机及原有 API 登录握手。地理位置测试使用固定 CIDR；正式部署前仍需用实际地区节点和官方客户端进行线路验收。
