# Fork 版本规则

当前版本：**1.1.16-beta1**；Git 标签：`1.1.16-beta1`；开发分支：`feature/server-fixapi`。上游版本系列：`1.1.16`。

- 版本号采用 `上游版本-betaN`，同一上游版本内的维护发布依次使用 beta1、beta2、beta3。
- 跟进并完成新上游版本的合并、测试后，改用新上游版本并从 beta1 开始。例如服务端 `1.1.16-beta3` → `1.1.17-beta1`。
- 上游发布新版本不会自动修改本 fork 的基线或版本号；以实际合并和验证为准。
- 已推送的版本标签不移动、不覆盖。修复通过新 beta 版本发布，已发布 beta1 不会因转为正式 Release 而改写标签。
- 普通开发提交不必每次增加 beta；准备一次新的版本发布时再递增。
- 三个仓库独立编号。API 的数据库迁移版本不是产品版本，不能随 beta 标签修改。Web 的 package.json/package-lock.json 和 Server 的 Cargo.toml/Cargo.lock 保持一致。
- API/Web 标签沿用上游 v 前缀；Server 沿用上游无 v 前缀。betaN 用作本 fork 的独立版本编号，不代表上游官方发行版；是否为预发布以 GitHub Release 的 prerelease 标记为准。经测试验收的 betaN 可以作为正式 Release 发布（prerelease=false）。

发布时记录配套 Web/API/Server 提交号。CI 验证功能分支及 beta 标签，不自动部署或创建 GitHub Release。

2026-09-27 发布约定：当前 beta1 已经用户验收，发布为正式 GitHub Release。只合并和发布，不自动部署。现有 beta1 标签保留原始已验证提交；后续仅文档和合并记录更新不移动标签。
