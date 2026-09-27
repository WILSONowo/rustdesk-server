# API 登录握手修复

基于官方 RustDesk Server 1.1.16，修复客户端登录 API 后连接 hbbs 的安全握手超时问题，具体实现见 [API_HANDSHAKE.md](API_HANDSHAKE.md)。

修改集中在 hbbs 的原生 TCP 握手和加密信令，hbbr 保持上游行为。不包含 API token 校验或强制登录功能，远控仍使用客户端原有的密码与授权机制。

项目保留上游 AGPL-3.0 许可证和版权声明。
