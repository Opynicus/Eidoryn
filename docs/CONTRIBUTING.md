# 贡献与提交

代码更改应从最新 `main` 创建独立特性分支，在分支上提交和验证后推送。由维护者审计分支并在 GitHub 发起 Pull Request，通过远端将变更合并回 `main`。避免直接在 `main` 上提交功能代码，也不要在本地创建合并提交后直接推送主分支。分支名可使用 `feature/`、`fix/` 等前缀。

提交信息采用 Conventional Commits，例如：

```text
feat(cpu): add shared device-cycle accounting
fix(cdrom): preserve interrupt timing
perf(gpu): reduce framebuffer copies
docs: clarify renderer options
test(cpu): cover interrupt boundaries
```

提交标题使用 `type(scope): description`；scope 可省略。Pull Request 的合并结果也应使用规范标题。在合并前执行与改动相关的测试；常规项目检查见[测试文档](TESTING.md)。
