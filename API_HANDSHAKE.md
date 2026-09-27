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

发布说明及 fork 工作流参见 [FORK_RELEASE.md](FORK_RELEASE.md)。保留原 AGPL-3.0 许可证及版权声明。
