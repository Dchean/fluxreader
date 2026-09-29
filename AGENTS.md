# 分支流向约定（FluxReader）

- `main`：纯项目代码主分支，**不含工作流内容**（无 `.workflow-kit/`、无 `WORKFLOW-KIT.md`）。
- `dev`：开发分支，承载 workflow-kit 工作流（状态、任务、决定）与全部开发过程记录。
- 一切调整先在 `dev` 提交、推送并验证，通过后再合并进 `main`；**合并不得把工作流内容带入 `main`**。

合并进 `main` 的标准步骤（在 `main` 上执行）：

```text
git merge --no-commit --no-ff dev
git rm -r -f -q .workflow-kit
git commit -m "merge: dev → main（不含工作流内容）"
```

`README.md` 两分支内容保持一致，合并冲突时取 `dev` 版；`AGENTS.md`、`CLAUDE.md`、`.gitignore`
含本分支专有内容，冲突时保留 `main` 版本。

发布：版本 bump 提交单独推送并等 CI 绿后，再在其上打 annotated tag（`v*`）——
tag 附注即 GitHub Release 正文（release.yml 自动提取），模板见 dev 的
`.workflow-kit/docs/RELEASE-NOTES-TEMPLATE.md`。
