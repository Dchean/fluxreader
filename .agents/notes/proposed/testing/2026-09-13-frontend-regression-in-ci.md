# Agent Note: 将现有前端逻辑回归接入 CI

Status: proposed

## Problem

`npm run test:frontend` 在 Windows/Node 24 上实测 8/8 通过（TASK-002 测量，本管理 agent 接手后复核仍为 8/8），但 `.github/workflows/ci.yml` 的前端作业只执行 `npm ci`、`npm run lint`、`npm run build`，从未调用它。

结果是：这 8 项覆盖流式翻译 XSS 回读消毒、全文提取失败可见性、翻译缓存命中等**状态机行为**的断言，只在有人手动运行时才生效。构建通过和 lint 通过都不能替代它们 —— 一个把 `translatedContent` 回读路径写错的改动，仍然可以通过 tsc 编译和 vite 打包。

ISSUE-002 记录的正是这个缺口。

## Proposal

在 `.github/workflows/ci.yml` 的 `frontend` 作业中，`Build (tsc + vite)` 之后新增一个步骤执行 `npm run test:frontend`，使用与既有步骤相同的失败语义（命令非 0 退出即作业失败）。

不新增 action、不引入新依赖、不修改触发器、不改 Node 版本、不扩大 token 权限、不设置 `continue-on-error`。

顺序放在 build 之后，是因为 `test:frontend` 依赖 `tsc -p tsconfig.test.json` 产出的 `dist-test/`；脚本自身的 `tsc -p tsconfig.test.json` 已自足，顺序不是硬依赖，放在 build 后只是为了让"编译失败"先于"行为断言失败"暴露，缩短诊断路径。

## Alternatives considered

### 不做：保持手动运行

最强理由是 CI 已经覆盖 lint 和 build，而前端回归是 Node 环境下的状态机检查，与真实桌面行为仍有距离；接入 CI 会增加每次 push 的耗时和一个新的失败面。

不采用的理由：8/8 已经证明该检查在受限环境中可稳定运行且无需外部服务，成本仅数秒。不接入等于让已存在的保护长期闲置，而这正是 ISSUE-002 记录的问题。

### 换成 vitest / jest 等真实测试框架再接入

最强理由是专用框架有更好的报告、并行和 watch 能力，长期更可维护。

不采用的理由：本任务是非目标明确写明的"不更换测试框架或加载器"。该脚本已可运行且有 8 项有效断言，先用最小改动取得 CI 保护；框架迁移涉及测试资产重写，属于独立任务，不应与门禁接入混在一起。

### 把 `test:frontend` 合并进 `npm run build` 或 `package.json` 的 `pretest`

最强理由是减少 YAML 改动，本地与 CI 走同一条命令。

不采用的理由：会把"编译"和"行为断言"耦合成一个不可分割的失败单元，CI 日志无法区分二者，且改变本地 `build` 的语义超出本任务范围（`package.json` 是受保护路径）。

## Acceptance criteria

- `frontend` 作业中存在明确执行 `npm run test:frontend` 的步骤，且失败会使作业失败。
- 保留既有 Node 版本（22）、触发器、`npm ci` / lint / build 三个步骤和 rust 作业。
- 无 `continue-on-error`、无 `|| true`、无 `set +e` 等吞掉退出码的写法。
- 无权限块新增，无第三方 action 新增。
- 本地执行 lint / test:frontend / build 通过；**托管 CI 本轮不运行**，其状态单独记录为 NOT_RUN，不得据此宣称 CI 已通过。

## Risks

- 本地为 Windows + Node 24，CI 前端为 Ubuntu + Node 22（ISSUE-010）。本地通过不等于 CI 通过。脚本使用 `--loader ./tools/test-loader.mjs`，Node 22 对 `--loader` 的支持与 Node 24 不同：Node 22 会打印 `ExperimentalWarning` 但仍可用；若未来 Node 升到移除该 flag 的版本，该步骤才会真正失败。本轮无法在本机复现 Node 22 环境，该风险如实记录，不伪称已在 CI 环境验证。
- 若 `dist-test/` 在 CI 上因路径大小写或行尾差异产出位置不同（`.gitattributes` 存在，需确认），步骤可能失败。这是**预期内的暴露**：失败应报告为真实的 CI 失败并单独修复，不得通过放宽步骤绕过。

## Verification

管理 agent 独立执行：

```text
git diff --check
npm run lint
npm run test:frontend
npm run build
```

并人工审查 YAML 的步骤顺序、缩进与失败语义；对托管 CI 记录 NOT_RUN。
