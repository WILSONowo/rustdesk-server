# API 登录握手兼容 fork

上游：`rustdesk/rustdesk-server`，基线 tag `1.1.16`（`73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`）。目标 fork：`WILSONowo/rustdesk-server`，分支 `feature/server-fixapi`。

详见 [API_HANDSHAKE.md](API_HANDSHAKE.md)。此分支仅补齐 hbbs 原生 TCP 的签名握手和加密信令处理；不替换 API、不校验账号 token、不强制远控登录。hbbr 保持上游行为。

保留上游 AGPL-3.0 许可证与版权声明，发布衍生镜像时同时提供匹配提交的源码和构建步骤。
