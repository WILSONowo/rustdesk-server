# Fork 版本规则

当前版本：**1.1.16-beta1**；Git 标签：`1.1.16-beta1`；开发分支：`feature/server-fixapi`。上游版本系列：`1.1.16`。

- 版本号采用 `上游版本-betaN`，同一上游版本内的维护发布依次使用 beta1、beta2、beta3。
- 跟进并完成新上游版本的合并、测试后，改用新上游版本并从 beta1 开始。例如服务端 `1.1.16-beta3` → `1.1.17-beta1`。
- 上游发布新版本不会自动修改本 fork 的基线或版本号；以实际合并和验证为准。
- 已推送的版本标签不移动、不覆盖。修复通过新 beta 版本发布。
- 普通开发提交不必每次增加 beta；准备一次新的版本发布时再递增。
- 三个仓库独立编号。API 的数据库迁移版本不是产品版本，不能随 beta 标签修改。Web 的 package.json/package-lock.json 和 Server 的 Cargo.toml/Cargo.lock 保持一致。
- API/Web 标签沿用上游 v 前缀；Server 沿用上游无 v 前缀。beta 表示本 fork 的预发布版本，不代表上游官方发行版。

发布时记录配套 Web/API/Server 提交号。CI 验证功能分支及 beta 标签，不自动部署或创建 GitHub Release。
