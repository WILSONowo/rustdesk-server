# API 登录握手

基于官方 `1.1.16`（`73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`），参考 forapi 分支和官方 RustDesk 1.4.9 的握手实现，补齐 hbbs 的 TCP 安全握手。

客户端配置公钥并登录 API 后，会等待服务端发送签名 KeyExchange。原版 hbbs 缺少这一步，会出现 `Failed to secure tcp: deadline has elapsed`。

实现内容：

- 使用现有 `id_ed25519` 签名，为每条连接生成独立临时密钥。
- 复用 hbb_common 的加密实现，让异步回复沿用连接的加密状态。
- 检查握手长度，拒绝重复握手、篡改、重放和加密后的明文降级。

未登录 API 的客户端仍可使用原有信令流程。UDP、WebSocket、管理／NAT 端口和 hbbr 保持原有行为，依赖版本未升级。

这项修复不校验 API token，也不限制账号只能控制指定设备。远控密码、确认授权和端到端加密仍由原协议处理。
