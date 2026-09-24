# 官方 1.1.16 的 API 登录握手兼容补丁

基线：官方 tag `1.1.16`，commit `73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`。
参考：lejianwen/rustdesk-server 的 forapi 分支，以及官方 RustDesk 1.4.9 的
`src/common.rs::secure_tcp_impl` / `create_symmetric_key_msg`。

## 修改范围

官方客户端在同时配置公钥、登录 API 账号时，会等待 hbbs 的签名 KeyExchange。
原版开源 hbbs 没有发送这个消息，导致 `Failed to secure tcp: deadline has elapsed`。
补丁增加签名握手及加密后的双向 TCP 协议处理，回复所用加密状态随异步响应的 sink 保存。

- 使用已有 `id_ed25519` 签名，每个连接独立生成临时 box 密钥。
- 复用官方 hbb_common 的 Encrypt，未修改依赖或 Cargo.lock。
- 检查握手长度，拒绝重复握手、篡改、重放和加密后的明文降级。
- 没登录 API 的官方 1.4.9 客户端可忽略握手提示，继续原有明文信令流程。
- 只改变 hbbs 主 TCP 监听；UDP、WebSocket、管理/NAT 端口及 hbbr 不改。
- 不移植 forapi 的 API 数据库、JWT、MUST_LOGIN 或日志中的密钥输出。

**兼容登录不等于鉴权授权。** 本补丁不验证 API token、不限制只有登录用户才能连接，
也不实现“每个用户只能控制分配设备”。原有客户端远控密码/授权与端到端加密仍由原协议负责。

## 本地构建和运行

需要 Docker Desktop Linux 容器和已初始化的 git submodule。

```powershell
git submodule update --init --recursive
./scripts/build-api-local.ps1
docker compose -f compose.api-local.yaml up -d
docker compose -f compose.api-local.yaml exec hbbs cat /root/id_ed25519.pub
```

本机 RustDesk 测试配置：

| 字段 | 值 |
| --- | --- |
| ID 服务器 | `127.0.0.1:21126` |
| 中继服务器 | `127.0.0.1:21127` |
| API 服务器 | 使用已有本地 API `http://127.0.0.1:21124`，或线上 `https://remote.example.com` |
| Key | 上述容器输出的本地测试公钥 |

本地独立 volume 生成测试密钥，不使用生产私钥。容器端口只绑定本机回环地址。
跨机器远控测试需另行开放到测试机 LAN 地址，并将 hbbs 的 `-r` 改成该 LAN 地址和 21127 端口，
两端都配置同一测试 ID 服务及公钥。仅一台客户端的本机测试无法覆盖真实双端远控。
测试结束恢复原客户端配置；`docker compose -f compose.api-local.yaml down` 保留测试数据。

## 验证范围

4 个单元测试覆盖签名、错误公钥、临时密钥独立性、多帧双向加密、畸形握手、
晚到/重复握手、篡改、重放、明文降级。
集成测试启动真实 hbbs 进程和独立临时数据库，以 1.4.9 同款算法模拟客户端，覆盖
未登录/已登录信令、UDP 注册、TCP 打洞异步回复、API 登录后的中继协商、WebSocket、管理端口。
通过 `HBBS_TEST_BINARY` 可对 release 二进制或原版 hbbs 运行同一集成测试。

这些是协议和服务进程测试，不等同于两台官方 GUI 客户端的实际桌面远控验收。

本地验证结果：4 个单元测试和 release 二进制集成测试均通过。
对线上同 digest 的原版 hbbs 运行同一握手测试，复现等待 KeyExchange 超时；
补丁版通过握手、打洞回复和中继协商。日志保存在忽略提交的 `artifacts/` 目录。

## 部署与发布

此补丁已通过真实服务进程测试，并验证过公网 IPv4/IPv6 加密握手；这不代替双端官方 GUI 的实际远控验收。
部署前备份 compose、镜像、密钥和一致性数据库副本，仅替换 hbbs。
沿用原持久化目录和公钥，保留原镜像以便回滚，hbbr 可继续使用官方 1.1.16 镜像。
不要将生产 `id_ed25519`、运行数据库或 SSH/SMTP 凭据加入仓库。

Linux 构建入口为 `sh scripts/build-api-local.sh`，Windows 为 `./scripts/build-api-local.ps1`。
两者都先构建和测试源码，再生成 `rustdesk-hbbs:1.1.16-beta1-local` 镜像；不会自动启动或更新服务器。
克隆时使用 `git clone --recurse-submodules`，已有克隆执行 `git submodule update --init --recursive`。
上游跟踪的 `.env` 仅含编译用 DATABASE_URL，`db_v2.sqlite3` 是 SQLx 编译用的上游数据库样本（含示例记录），不是本次部署的运行数据。

发布说明及 fork 工作流参见 [FORK_RELEASE.md](FORK_RELEASE.md)。保留原 AGPL-3.0 许可证及版权声明。
