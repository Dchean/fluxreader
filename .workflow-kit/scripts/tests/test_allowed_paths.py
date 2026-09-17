"""TASK-046 回归自测：prepare 的 allowed_paths 解析。

覆盖五种 spec 写法。缺口背景见 docs/TOOL-GAP-prepare-allowed-paths.md：
该缺口两次静默致阻塞（TASK-043、TASK-044），故必须有可复跑的回归测试。

运行：python .workflow-kit/scripts/tests/test_allowed_paths.py
退出码 0 = 全部通过。
"""
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))

import workflow_runtime as wr  # noqa: E402

FALLBACK = ["src-tauri/src", "src-tauri/tests"]
CASES = []


def case(name):
    def deco(fn):
        CASES.append((name, fn))
        return fn
    return deco


@case("仅顶层 allowed_paths → 采用顶层值")
def _top_only():
    spec = {"allowed_paths": ["src/**"], "snapshot_paths": FALLBACK}
    assert wr.resolve_allowed_paths(spec, FALLBACK) == ["src/**"]


@case("仅 scope.allowed_paths → 采用嵌套值（本次修复点）")
def _nested_only():
    spec = {"scope": {"allowed_paths": ["src/**"]}, "snapshot_paths": FALLBACK}
    got = wr.resolve_allowed_paths(spec, FALLBACK)
    assert got == ["src/**"], f"嵌套值被忽略，得到 {got!r}（即静默回退到 snapshot_paths）"


@case("两处一致 → 采用该值")
def _both_same():
    spec = {"allowed_paths": ["a/**"], "scope": {"allowed_paths": ["a/**"]}}
    assert wr.resolve_allowed_paths(spec, FALLBACK) == ["a/**"]


@case("两处冲突 → 抛错，不静默择一")
def _conflict():
    spec = {"allowed_paths": ["a/**"], "scope": {"allowed_paths": ["b/**"]}}
    try:
        wr.resolve_allowed_paths(spec, FALLBACK)
    except ValueError as exc:
        assert "不一致" in str(exc), f"报错信息未说明冲突：{exc}"
        return
    raise AssertionError("两处冲突时未抛错（会静默择一）")


@case("两处都没有 → 回退 snapshot_paths（保持兼容）")
def _fallback():
    spec = {"snapshot_paths": FALLBACK}
    assert wr.resolve_allowed_paths(spec, FALLBACK) == FALLBACK


@case("scope 不是 dict（异常输入）→ 不崩溃，按顶层/回退处理")
def _scope_not_dict():
    spec = {"scope": "oops", "allowed_paths": ["a/**"]}
    assert wr.resolve_allowed_paths(spec, FALLBACK) == ["a/**"]
    assert wr.resolve_allowed_paths({"scope": "oops"}, FALLBACK) == FALLBACK


@case("结果必须是深拷贝（改返回值不得影响 spec）")
def _deepcopy():
    spec = {"allowed_paths": ["a/**"]}
    got = wr.resolve_allowed_paths(spec, FALLBACK)
    got.append("mutated")
    assert spec["allowed_paths"] == ["a/**"], "返回值与 spec 共享引用"


def main():
    failed = 0
    for name, fn in CASES:
        try:
            fn()
        except Exception as exc:  # noqa: BLE001
            print(f"  FAIL  {name}\n          {type(exc).__name__}: {exc}")
            failed += 1
        else:
            print(f"  PASS  {name}")
    total = len(CASES)
    print(f"\n  {total - failed}/{total} 通过")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
