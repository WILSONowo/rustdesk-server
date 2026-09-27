# API 登录握手兼容 fork

上游：`rustdesk/rustdesk-server`，基线 tag `1.1.16`（`73523b31cfd25d77dee862e6fc9f5e1fb5e485ef`）。目标 fork：`WILSONowo/rustdesk-server`，分支 `feature/server-fixapi`。

详见 [API_HANDSHAKE.md](API_HANDSHAKE.md)。此分支仅补齐 hbbs 原生 TCP 的签名握手和加密信令处理；不替换 API、不校验账号 token、不强制远控登录。hbbr 保持上游行为。

```sh
git submodule update --init --recursive
sh scripts/build-api-local.sh
```

Windows 使用 `./scripts/build-api-local.ps1`。需要 Docker Linux 容器。脚本运行 4 个协议单元测试、release hbbs 构建和真实进程集成测试，再生成本地镜像；不会更新任何运行容器。

`artifacts/hbbs` 与运行密钥/数据库不提交。仓库内上游跟踪的 `db_v2.sqlite3` 是编译用数据库样本（含上游示例记录），`.env` 只包含其 DATABASE_URL；构建时复制到临时路径，不读取生产数据。Linux amd64 是当前验证平台，其他架构须单独验证。

`.github/workflows/verify.yml` 使用同一 Linux 构建脚本验证，不推镜像、不创建 Release。原上游发布流程存放在 `.github/legacy-workflows/`，避免沿用上游镜像命名和发布目标。

生产替换前备份旧镜像、compose、密钥和一致性 SQLite 快照，沿用原数据目录，只替换 hbbs。回滚使用保存的镜像，不能重新生成服务器私钥。

保留上游 AGPL-3.0 许可证与版权声明，发布衍生镜像时同时提供匹配提交的源码和构建步骤。
