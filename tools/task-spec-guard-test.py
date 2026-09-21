#!/usr/bin/env python3
"""TASK-080：prepare 前置守卫的独立可复跑自测。

运行：
    python tools/task-spec-guard-test.py

退出码 0 = 全部通过；非 0 = 有失败。

覆盖意图承接自 .workflow-kit/scripts/tests/test_allowed_paths.py（TASK-046 的回归测试）：
该文件位于受保护的 .workflow-kit/scripts/** 内，任何任务不得修改；且它依赖的引擎函数
resolve_allowed_paths 已被 commit 6fd382e「rebind --upgrade-tools」的升级整段覆盖，
当前实测 0/7 通过、exit 1。在不改引擎的约束下，其 7 项覆盖意图由本文件承接。

本文件只调用守卫暴露的纯函数与用例表，不复制判定逻辑——保证「测的就是跑的」。
另外还对守卫做端到端 CLI 检查（临时 spec 文件 → 退出码），覆盖「被拒绝时不放行」这一
最关键的失败路径。
"""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
GUARD = HERE / "task-spec-guard.py"


def load_guard():
    """按文件路径加载 tools/task-spec-guard.py（文件名含连字符，不能直接 import）。"""
    spec = importlib.util.spec_from_file_location("task_spec_guard", GUARD)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"无法加载守卫脚本：{GUARD}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_unit_cases(guard) -> tuple[int, int]:
    """跑守卫内置用例表，返回 (通过数, 总数)。"""
    passed = 0
    total = 0
    for name, fn in guard._CASES:
        total += 1
        try:
            fn()
            print(f"  PASS  用例：{name}")
            passed += 1
        except Exception as error:  # noqa: BLE001 - 自测必须如实报告任何失败
            print(f"  FAIL  用例：{name}")
            print(f"          {type(error).__name__}: {error}")
    return passed, total


def run_cli_checks(guard) -> tuple[int, int]:
    """端到端检查：合法的放行、非法的拦下（退出码语义）。"""
    passed = 0
    total = 0

    cases = [
        ("合法（顶层写法）应放行", {"allowed_paths": ["src/**"], "snapshot_paths": ["src"]}, 0),
        ("非法（仅嵌套）应拦下", {"scope": {"allowed_paths": ["src/**"]}, "snapshot_paths": ["src"]}, 1),
        ("非法（缺 allowed_paths）应拦下", {"snapshot_paths": ["src"]}, 1),
        ("非法（顶层与嵌套冲突）应拦下",
         {"allowed_paths": ["src/**"], "scope": {"allowed_paths": ["tools/**"]}, "snapshot_paths": ["src"]}, 1),
    ]
    for label, payload, expected in cases:
        total += 1
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "spec.json"
            p.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
            proc = subprocess.run([sys.executable, str(GUARD), str(p)],
                                  capture_output=True, text=True, encoding="utf-8")
        if proc.returncode == expected:
            print(f"  PASS  CLI：{label}（退出码 {proc.returncode}）")
            passed += 1
        else:
            print(f"  FAIL  CLI：{label}（期望退出码 {expected}，实际 {proc.returncode}）")
            if proc.stdout:
                print("          stdout:", proc.stdout.strip().splitlines()[-1][:120])
    return passed, total


def main() -> int:
    if not GUARD.exists():
        print(f"[错误] 找不到守卫脚本：{GUARD}")
        return 2
    guard = load_guard()

    print("== 单元用例（判定语义）==")
    up, ut = run_unit_cases(guard)

    print("\n== 端到端 CLI（退出码语义）==")
    cp, ct = run_cli_checks(guard)

    passed, total = up + cp, ut + ct
    print(f"\n  合计 {passed}/{total} 通过")
    if passed == total:
        print("  OK：守卫判定与拦截行为符合预期。")
        return 0
    print("  FAILED：存在不符合预期的用例。")
    return 1


if __name__ == "__main__":
    sys.exit(main())
