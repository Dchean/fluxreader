#!/usr/bin/env python3
"""TASK-080：prepare 前置守卫 —— 在把 spec 交给 project_workflow.py prepare 之前拦下会被静默忽略的写法。

背景（缺口与根因，详见 .workflow-kit/docs/TOOL-GAP-prepare-allowed-paths.md）：
  workflow_runtime.prepare 只从 spec 的**顶层**读 allowed_paths：

      task["scope"]["allowed_paths"] = copy.deepcopy(specification.get("allowed_paths", paths))

  而项目自带的任务**记录**模板 .workflow-kit/tasks/templates/TASK.json 把该字段放在
  **scope.allowed_paths**（嵌套）。若照记录模板的形状书写 prepare 用的 spec，
  工具不会报错、不会警告，直接走默认值 snapshot_paths —— spec 里声明的范围被**静默丢弃**。
  该缺口历史上两次致阻塞（TASK-043、TASK-044），当时靠 owner 授权的台账订正解锁。

  （注：spec 模板 .workflow-kit/tasks/templates/TASK-SPEC.json 用的是正确的**顶层**写法；
    误用主要发生在照 TASK.json 记录模板书写时。升级后 matches() 已支持目录前缀，
    故「目录名匹配不到文件 → 全部越界」的死锁前提在**当前**版本已不成立，
    但「范围声明被静默篡改」依然存在，这才是本守卫要拦的。）

为什么放在工程侧而不是改引擎：
  引擎脚本（.workflow-kit/scripts/**）属引擎托管、且位于任务模板默认 protected_paths 内，
  任何任务都不得修改；历史上曾以总控级基础设施提交打过补丁（commit a267b1e 的
  resolve_allowed_paths），但 commit 6fd382e「rebind --upgrade-tools」升级时把它整段覆盖，
  补丁失效而回归测试留在原地静默失败。故本守卫放在**项目自有**的 tools/ 下，升级不会覆盖。

用法：
  python tools/task-spec-guard.py <spec.json> [<spec2.json> ...]
  python tools/task-spec-guard.py --self-test      # 运行内置自检

退出码：
  0 = 全部 spec 合法（可安全交给 prepare）
  1 = 有不合法 spec（已逐条打印原因与修复方法）
  2 = 用法/IO 错误
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

EXIT_OK = 0
EXIT_INVALID = 1
EXIT_USAGE = 2


def resolve_allowed_paths(spec: dict, fallback):
    """复刻并加固 prepare 的 allowed_paths 取值语义（只读，不改引擎）。

    返回 (paths, problems, notes)：
      paths    —— 按当前引擎语义最终会被记录的 allowed_paths（用于如实展示后果）
      problems —— 必须阻止 prepare 的问题（致命）
      notes    —— 提示性信息（不阻止）

    致命情形：
      · allowed_paths 只写在嵌套 scope.allowed_paths → 会被引擎**静默忽略**
      · 顶层与嵌套同时给出且不一致 → 两份真相，必须由作者明确择一
      · allowed_paths 缺失或为空 → 引擎会回退 snapshot_paths（范围声明落空）
      · allowed_paths 含非字符串元素
    合法情形：只给顶层（引擎读取它）。
    """
    problems: list[str] = []
    notes: list[str] = []

    top = spec.get("allowed_paths")
    scope = spec.get("scope")
    nested = scope.get("allowed_paths") if isinstance(scope, dict) else None

    def check_list(value, label):
        if not isinstance(value, list) or not value or not all(isinstance(x, str) and x.strip() for x in value):
            problems.append(f"{label} 必须是非空的字符串列表（当前：{value!r}）")
            return False
        return True

    if top is not None and nested is not None:
        # 两处都给：一致可接受（不静默择一即可），不一致必须让作者决定
        if not check_list(top, "顶层 allowed_paths"):
            pass
        elif not check_list(nested, "scope.allowed_paths"):
            pass
        elif list(top) != list(nested):
            problems.append(
                "顶层 allowed_paths 与 scope.allowed_paths 同时给出且不一致；"
                "引擎只读顶层，嵌套值会被静默忽略。请删掉嵌套写法并只保留顶层（或改为一致）"
            )
        else:
            notes.append("顶层与 scope.allowed_paths 一致；引擎读顶层，结果无歧义")
        effective = top
    elif top is not None:
        if check_list(top, "顶层 allowed_paths"):
            effective = top
        else:
            effective = None
    elif nested is not None:
        # 核心缺口：只写嵌套 → 引擎读不到
        check_list(nested, "scope.allowed_paths")
        problems.append(
            "allowed_paths 只写在嵌套 scope.allowed_paths；workflow_runtime.prepare 只读**顶层**，"
            "该声明会被静默忽略并回退成 snapshot_paths。请把它移到 spec 的顶层 allowed_paths"
        )
        effective = None
    else:
        problems.append(
            "spec 未提供 allowed_paths（顶层）；引擎会回退 snapshot_paths，"
            "任务范围将不是你所声明的。请补顶层 allowed_paths"
        )
        effective = None

    # snapshot_paths 仍需合法：它决定候选快照覆盖范围
    snap = spec.get("snapshot_paths")
    if not isinstance(snap, list) or not snap or not all(isinstance(x, str) and x.strip() for x in snap):
        problems.append("snapshot_paths 必须是非空的字符串列表（决定候选快照范围）")

    if effective is None and not problems:
        effective = fallback
    # 深拷贝返回：调用方改返回值不得影响入参 spec（原 test_allowed_paths.py 的第 7 项意图）
    return (list(effective) if isinstance(effective, list) else effective), problems, notes


def guard(path: Path) -> bool:
    """校验单个 spec 文件；合法返回 True。"""
    try:
        spec = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        print(f"[错误] 文件不存在：{path}")
        return False
    except json.JSONDecodeError as error:
        print(f"[错误] {path} 不是合法 JSON：{error}")
        return False

    if not isinstance(spec, dict):
        print(f"[错误] {path} 顶层必须是 JSON 对象")
        return False

    # 记录模板形状的误用：整个 scope 被搬进 spec（常见复制粘贴事故）
    effective, problems, notes = resolve_allowed_paths(spec, spec.get("snapshot_paths"))

    name = spec.get("id") or path.name
    if problems:
        print(f"[拒绝] {name}（{path}）")
        for p in problems:
            print(f"        · {p}")
        print("        修好后再运行 project_workflow.py prepare --file " + str(path))
        return False

    print(f"[通过] {name}（{path}）")
    print(f"        记录后 allowed_paths = {effective}")
    for n in notes:
        print(f"        注：{n}")
    return True


# ----------------------------------------------------------------------------
# 内置自检：承接原 .workflow-kit/scripts/tests/test_allowed_paths.py 的覆盖意图
# （该测试位于受保护的 .workflow-kit/scripts/** 内、任何任务不得修改；
#   且其依赖的引擎函数 resolve_allowed_paths 已被 commit 6fd382e 的升级覆盖，
#   当前 0/7 通过。其 7 项覆盖意图由本自检承接。）
# ----------------------------------------------------------------------------
FALLBACK = ["src-tauri/src", "src-tauri/tests"]

_CASES = []


def case(name):
    def deco(fn):
        _CASES.append((name, fn))
        return fn
    return deco


@case("仅顶层 allowed_paths → 通过，采用顶层值")
def _top_only():
    spec = {"allowed_paths": ["src/**"], "snapshot_paths": FALLBACK}
    paths, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert paths == ["src/**"], paths
    assert not problems, problems


@case("仅 scope.allowed_paths → 拒绝（本次修复点的核心缺口）")
def _nested_only():
    spec = {"scope": {"allowed_paths": ["src/**"]}, "snapshot_paths": FALLBACK}
    _, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert problems, "只写嵌套必须被拒绝"
    assert any("静默忽略" in p for p in problems), problems


@case("顶层与嵌套一致 → 通过（无歧义）")
def _both_same():
    spec = {"allowed_paths": ["src/**"], "scope": {"allowed_paths": ["src/**"]},
            "snapshot_paths": FALLBACK}
    paths, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert paths == ["src/**"], paths
    assert not problems, problems


@case("顶层与嵌套冲突 → 拒绝（不静默择一）")
def _conflict():
    spec = {"allowed_paths": ["src/**"], "scope": {"allowed_paths": ["tools/**"]},
            "snapshot_paths": FALLBACK}
    _, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert problems, "冲突必须被拒绝"
    assert any("不一致" in p for p in problems), problems


@case("两处都没有 → 拒绝并提示会回退 snapshot_paths")
def _none():
    spec = {"snapshot_paths": FALLBACK}
    _, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert problems, "缺失必须被拒绝"
    assert any("回退 snapshot_paths" in p for p in problems), problems


@case("scope 不是 dict → 不崩溃，按顶层/缺失处理")
def _scope_not_dict():
    spec = {"scope": "not-a-dict", "snapshot_paths": FALLBACK}
    _, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert problems, "无顶层 allowed_paths 时必须报缺失"
    # 且给出的是「缺失」而非崩溃
    assert any("未提供 allowed_paths" in p for p in problems), problems

    spec2 = {"scope": "not-a-dict", "allowed_paths": ["src/**"], "snapshot_paths": FALLBACK}
    paths, problems2, _ = resolve_allowed_paths(spec2, FALLBACK)
    assert paths == ["src/**"] and not problems2, (paths, problems2)


@case("snapshot_paths 缺失 → 拒绝（候选快照范围不可为空）")
def _no_snapshot():
    spec = {"allowed_paths": ["src/**"]}
    _, problems, _ = resolve_allowed_paths(spec, None)
    assert any("snapshot_paths" in p for p in problems), problems


@case("allowed_paths 含非字符串元素 → 拒绝")
def _bad_element():
    spec = {"allowed_paths": ["src/**", 42], "snapshot_paths": FALLBACK}
    _, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert problems, "非字符串元素必须被拒绝"


@case("结果必须是深拷贝语义：改返回值不影响 spec")
def _deep_copy():
    spec = {"allowed_paths": ["src/**"], "snapshot_paths": FALLBACK}
    paths, problems, _ = resolve_allowed_paths(spec, FALLBACK)
    assert not problems, problems
    paths.append("mutated")
    assert spec["allowed_paths"] == ["src/**"], "spec 被返回值影响"


def self_test() -> int:
    passed = 0
    for name, fn in _CASES:
        try:
            fn()
            print(f"  PASS  {name}")
            passed += 1
        except Exception as error:  # noqa: BLE001 - 自检要如实报告任何失败
            print(f"  FAIL  {name}")
            print(f"          {type(error).__name__}: {error}")
    print(f"\n  {passed}/{len(_CASES)} 通过")
    return EXIT_OK if passed == len(_CASES) else EXIT_INVALID


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return EXIT_USAGE
    if argv[0] == "--self-test":
        return self_test()
    ok = True
    for raw in argv:
        ok = guard(Path(raw)) and ok
    if not ok:
        print("\n有一只或多只 spec 被拒绝；修正后再运行 prepare（避免范围静默回退）。")
    return EXIT_OK if ok else EXIT_INVALID


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
