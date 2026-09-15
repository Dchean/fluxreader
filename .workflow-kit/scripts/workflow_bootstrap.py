"""Verifiable, non-destructive workflow attachment; no model or application calls."""
# Note: explicit integration and isolated records — ../.agents/notes/implemented/process/
# 2026-09-13-portable-project-workflow.md; installed copy: ../docs/workflow/design-note.md.
from __future__ import annotations

import hashlib
import json
import os
import re
import uuid
from pathlib import Path

import project_workflow as w

ENTRY_START = "<!-- workflow-kit: entry -->"
ENTRY_END = "<!-- /workflow-kit: entry -->"
ENGINE_NAMES = ("project_workflow.py", "workflow_runtime.py", "workflow_bootstrap.py", "workflow_intake.py", "workflow_progress.py")
LEGACY_NAMES = ("AGENTS.md", "CLAUDE.md", "WORKFLOW.md", "tasks/PROJECT.json", "tasks/POLICY.json",
                "tasks/EXECUTION-POLICY.json", "docs/HANDOFF.md", "docs/PROCESS.md",
                "docs/EXECUTION-CONTRACT.md", "docs/PRODUCT.md", "docs/FEATURES.md", "docs/ARCHITECTURE.md")
_HASH_CACHE = {}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def managed_digest(name, data):
    if Path(name).suffix.lower() in {".py", ".md", ".json", ".yaml", ".yml", ".txt"}:
        data = data.replace(b"\r\n", b"\n")
    return sha(data)


def file_sha(path):
    """Reuse hashes only while the file metadata is unchanged in this process."""
    stat = path.stat()
    key = (stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns, stat.st_ino)
    cached = _HASH_CACHE.get(str(path))
    if cached is None or cached[0] != key:
        cached = (key, managed_digest(path.name, path.read_bytes()))
        _HASH_CACHE[str(path)] = cached
    return cached[1]


def entry_digest(data):
    text = data.decode("utf-8-sig").replace("\r\n", "\n")
    return sha((text.split(ENTRY_END, 1)[0] + ENTRY_END).encode("utf-8"))


def tool_digest():
    folder = Path(__file__).resolve().parent
    return w.digest({name: file_sha(folder / name) for name in ENGINE_NAMES})


def entry_header(entry, helper):
    return (ENTRY_START + "\n# 当前工作流：workflow-kit\n\n"
            "当前主会话是总控；明确的 task_id/run_id 指定实现任务，role=reviewer + task_id/candidate_digest 指定审查任务。\n"
            f"先读 [{entry}]({entry})，从项目根目录运行 `python {helper} start --root .` 并报告接入状态。\n"
            "用户当前明确选择的流程优先于旧流程入口；原有业务、数据、安全和兼容约束继续核对。\n"
            "旧任务和旧授权是历史材料，不自动决定本次目标、执行器或当前角色；未解决的冲突必须列明。\n"
            "已有项目先问重构意向，分析后提问让用户选路线；明确询问 Agent 全部处理或 Agent + CLI，保存真实回答。\n"
            "必须向用户展示当前阶段目标、任务状态、阻塞与下一步；使用真实原生任务面板，或直接展示 progress 的 Markdown。\n"
            "此入口不能覆盖宿主系统约束，也不授予业务代码、付费调用或发布权限。\n"
            + ENTRY_END + "\n\n")


def prepend_entry(original, entry, helper):
    text = original.decode("utf-8-sig")
    if ENTRY_START in text or "<!-- project-workflow: entry -->" in text:
        raise ValueError("An existing workflow bridge needs explicit migration; do not stack another entry")
    bom = b"\xef\xbb\xbf" if original.startswith(b"\xef\xbb\xbf") else b""
    body = original[len(bom):]
    return bom + entry_header(entry, helper).encode("utf-8") + body


def make_binding(plan, layout, source, host, legacy_sources=()):
    prefix = w.ISOLATED_ROOT + "/" if layout == "isolated" else ""
    entry = "WORKFLOW-KIT.md" if prefix else "WORKFLOW.md"
    managed = {name: managed_digest(name, data) for name, data in plan.items()
               if name.startswith((prefix + "scripts/", prefix + "docs/workflow/", prefix + "tasks/templates/"))
               or name in {entry, prefix + "WORKFLOW.md"}}
    return {"schema_version": 1, "package": "workflow-kit", "layout": layout,
            "installed_at_utc": w.iso(w.utc_now()), "source": source, "host_hint": host,
            "entry": entry, "project_file": prefix + "tasks/PROJECT.json",
            "helper": prefix + "scripts/project_workflow.py", "tool_digest": tool_digest(),
            "entry_digests": {name: entry_digest(plan[name]) for name in ("AGENTS.md", "CLAUDE.md") if name in plan},
            "managed_files": managed, "legacy_sources": list(legacy_sources),
            "note": "File consistency is verifiable; this is not proof of model compliance or application authorization."}


def classic_binding(plan, source="init: workflow records only", legacy_sources=()):
    return make_binding(plan, "classic", source, "current_agent", legacy_sources)


def binding_record(root):
    found = [name for name in (w.ISOLATED_BINDING, w.CLASSIC_BINDING) if w.inside(root, name).is_file()]
    if len(found) > 1:
        raise ValueError("Two workflow-kit bindings found; reconcile them before continuing")
    return (found[0], w.read_json(w.inside(root, found[0]))) if found else (None, None)


def integration_status(root):
    root = Path(root).resolve()
    result = {"connected": False, "root": str(root), "status": "not_connected", "errors": []}
    try:
        marker, binding = binding_record(root)
        if binding is None:
            if w.inside(root, w.ISOLATED_ROOT).exists():
                result.update(status="incomplete_or_foreign_namespace", errors=["Inspect the existing .workflow-kit directory; it has no valid binding"])
            elif w.inside(root, "tasks/PROJECT.json").exists():
                project = w.read_json(w.inside(root, "tasks/PROJECT.json"))
                likely_kit = project.get("kind") in {"new", "refactor"} and bool(project.get("policy"))
                result.update(status="legacy_kit_needs_migration" if likely_kit else "foreign_workflow",
                              legacy_project="tasks/PROJECT.json",
                              errors=["PROJECT.json alone does not identify workflow-kit; do not resume or overwrite it"])
            return result
        if binding.get("schema_version") != 1 or binding.get("package") != "workflow-kit" or binding.get("layout") not in {"classic", "isolated"}:
            raise ValueError("Unsupported workflow-kit binding")
        prefix = w.ISOLATED_ROOT + "/" if binding["layout"] == "isolated" else ""
        expected_marker = w.ISOLATED_BINDING if prefix else w.CLASSIC_BINDING
        if marker != expected_marker or binding.get("project_file") != prefix + "tasks/PROJECT.json" or binding.get("helper") != prefix + "scripts/project_workflow.py":
            raise ValueError("Binding paths do not match the declared layout")
        managed = binding.get("managed_files")
        if not isinstance(managed, dict) or any(prefix + "scripts/" + name not in managed for name in ENGINE_NAMES):
            raise ValueError("Binding lacks its managed engine files")
        for name, expected in managed.items():
            path = w.inside(root, name)
            if not path.is_file() or file_sha(path) != expected:
                result["errors"].append("Managed workflow file changed/missing: " + name)
        for name in ("AGENTS.md", "CLAUDE.md"):
            path = w.inside(root, name)
            text = path.read_text(encoding="utf-8-sig") if path.is_file() else ""
            if not text.startswith(ENTRY_START) or ENTRY_END not in text[:3000]:
                result["errors"].append("Current workflow entry is not at the top of " + name)
            elif entry_digest(path.read_bytes()) != binding.get("entry_digests", {}).get(name):
                result["errors"].append("The active workflow entry changed: " + name)
        if binding.get("tool_digest") != tool_digest():
            result["errors"].append("The calling tools differ from the installed version; review an upgrade instead of replacing records")
        project = w.read_json(w.inside(root, binding["project_file"]))
        if project.get("workflow_kit") != "workflow-kit":
            result["errors"].append("The selected PROJECT is not marked as workflow-kit state")
        result.update(status="connected" if not result["errors"] else "integration_needs_repair",
                      connected=not result["errors"], layout=binding["layout"], entry=binding["entry"],
                      project_file=binding["project_file"], helper=binding["helper"], binding_file=marker,
                      host_hint=binding.get("host_hint"), current_role="manager unless explicitly delegated a worker task packet",
                      stage=project.get("stage"), current_task=project.get("current_task"),
                      verified_managed_files=len(managed), tool_digest=binding.get("tool_digest"),
                      legacy_sources=binding.get("legacy_sources", []), authority_source=prefix + "tasks/POLICY.json",
                      integration_does_not_grant_code_authority=True)
        return result
    except (ValueError, OSError, KeyError, TypeError) as error:
        result.update(status="integration_needs_repair")
        result["errors"].append(str(error))
        return result


def render_isolated_markdown(data):
    """Keep relative Markdown links in the moved tree; qualify command/record paths."""
    links = []
    def protect(match):
        links.append(match.group(0))
        return "\x00LINK" + str(len(links) - 1) + "\x00"
    text = re.sub(r"!?\[[^\]\n]*\]\([^\n)]+\)", protect, data.decode("utf-8-sig"))
    for value in ("scripts/project_workflow.py", "tasks/", "docs/"):
        text = re.sub(r"(?<!" + re.escape(w.ISOLATED_ROOT + "/") + ")" + re.escape(value), w.ISOLATED_ROOT + "/" + value, text)
    text = re.sub(r"(?<![/\w.-])WORKFLOW\.md", "WORKFLOW-KIT.md", text)
    for index, value in enumerate(links):
        text = text.replace("\x00LINK" + str(index) + "\x00", value)
    return text.encode("utf-8")


def bootstrap(root, kind="refactor", name=None, source=None, host="current_agent", write=False, full_docs=False):
    root = Path(root).resolve()
    current = integration_status(root)
    if current["connected"]:
        return {"ok": True, "written": False, "integration": current, "next": "start"}
    if current["status"] not in {"not_connected", "foreign_workflow"}:
        return {"ok": False, "written": False, "integration": current, "next": "repair_integration"}
    package = Path(w.__file__).resolve().parent.parent
    if not (package / "assets/project").is_dir():
        raise ValueError("Use bootstrap from the full startup package")
    if root == package or (root == package.parent and (root / "START.md").is_file()):
        raise ValueError("The startup package is not the target application directory")
    prefix = w.ISOLATED_ROOT + "/"
    if w.inside(root, "WORKFLOW-KIT.md").exists():
        raise ValueError("Existing WORKFLOW-KIT.md has no valid binding; inspect it instead of overwriting")
    base = w.initialization_plan(root, kind, name or root.name, allow_existing=True, full_docs=full_docs)
    base.pop(w.CLASSIC_BINDING, None)
    project = json.loads(base["tasks/PROJECT.json"])
    project.update(policy=prefix + "tasks/POLICY.json", handoff=prefix + "docs/HANDOFF.md")
    base["tasks/PROJECT.json"] = w.encoded(project)
    task = json.loads(base["tasks/templates/TASK.json"])
    task["scope"]["protected_paths"] += ["WORKFLOW-KIT.md", prefix + "binding.json", prefix + "tasks/**",
        prefix + "scripts/**", prefix + "docs/workflow/**", prefix + "legacy/**", prefix + "AGENTS.md", prefix + "CLAUDE.md",
        prefix + "WORKFLOW.md", prefix + "docs/PRODUCT.md", prefix + "docs/FEATURES.md"]
    base["tasks/templates/TASK.json"] = w.encoded(task)
    plan = {prefix + rel: render_isolated_markdown(data) if rel.endswith(".md") else data for rel, data in base.items()}
    legacy = [name for name in LEGACY_NAMES if w.inside(root, name).is_file()]
    # Existing business documents remain the source; small links avoid copying their facts.
    for rel in ("docs/PRODUCT.md", "docs/FEATURES.md", "docs/ARCHITECTURE.md", "docs/BASELINE.md", "docs/UI.md"):
        if w.inside(root, rel).is_file():
            plan[prefix + rel] = ("# 沿用原项目资料\n\n请读取 [原项目文档](../../" + rel + ")。\n\n"
                                 "保留其中的业务/兼容约束，区分历史测量与本次目标；旧执行流程和授权不自动延续。\n").encode("utf-8")
    lines = ["# workflow-kit 当前项目入口", "", "从项目根目录运行：", "",
             "```text", "python .workflow-kit/scripts/project_workflow.py start --root .", "```", "",
             "先报告 integration.connected、目标目录、当前阶段和任务路径；未接入或校验失败时先排障，不能静默转回旧流程。", "",
             "当前主会话是总控；Worker 身份只来自明确的任务执行包。接入文件不代表业务改造已经获批。", "",
             "- [当前状态](.workflow-kit/tasks/PROJECT_STATE.md)", "- [新流程](.workflow-kit/WORKFLOW.md)",
             "- [问答](.workflow-kit/docs/workflow/INTAKE.md)", "", "## 原项目资料与冲突处理", "",
             "以下资料的产品、数据、安全和兼容约束继续核对；旧的角色、任务源、启动顺序不能静默覆盖用户当前选择。", "",
             "onboard 前在 legacy_review 记录已读来源、保留约束、旧任务处置和流程冲突结论。新目标不重置旧任务预算，也不借旧批准自动开始新的工作。", ""]
    lines += ["- [" + rel + "](" + rel + ")" for rel in legacy]
    lines += ["", "WorkBuddy 或其他宿主若没有自动读取项目入口，在新会话首条消息明确要求读取本文件并运行 start；不要假定某个厂商会自动加载所有 Markdown。", ""]
    plan["WORKFLOW-KIT.md"] = "\n".join(lines).encode("utf-8")
    originals = {}
    for rel in ("AGENTS.md", "CLAUDE.md"):
        path = w.inside(root, rel)
        before = path.read_bytes() if path.is_file() else b""
        originals[rel] = before if path.is_file() else None
        if before:
            plan[prefix + "legacy/" + rel] = before
        plan[rel] = prepend_entry(before, "WORKFLOW-KIT.md", prefix + "scripts/project_workflow.py")
    binding = make_binding(plan, "isolated", source, host, legacy)
    plan[w.ISOLATED_BINDING] = w.encoded(binding)
    result = {"ok": True, "written": False, "integration": current, "files": sorted(plan),
              "preserved_legacy_sources": legacy, "prepended_entries": [key for key, value in originals.items() if value is not None],
              "application_work_authorized": False, "next": "bootstrap --write --source with the actual workflow selection"}
    if not write:
        return result
    if not source or not source.strip():
        raise ValueError("Record the actual user's workflow selection in --source; this does not authorize product changes")
    if not root.parent.is_dir():
        raise ValueError("The target parent must already exist")
    created, directories, replaced = [], [], []
    try:
        for rel, data in plan.items():
            path = w.inside(root, rel)
            missing, parent = [], path.parent
            while not parent.exists():
                missing.append(parent)
                parent = parent.parent
            for directory in reversed(missing):
                directory.mkdir()
                directories.append(directory)
            if rel in originals and originals[rel] is not None:
                if path.read_bytes() != originals[rel]:
                    raise ValueError("An entrypoint changed during integration: " + rel)
                temporary = path.with_name(path.name + ".workflow-" + uuid.uuid4().hex + ".tmp")
                try:
                    w.write_exclusive(temporary, data)
                    if path.read_bytes() != originals[rel]:
                        raise ValueError("An entrypoint changed before replacement: " + rel)
                    os.replace(temporary, path)
                    replaced.append(rel)
                finally:
                    temporary.unlink(missing_ok=True)
            else:
                w.write_exclusive(path, data)
                created.append(path)
        status = integration_status(root)
        if not status["connected"]:
            raise ValueError("; ".join(status["errors"]))
    except Exception:
        for rel in reversed(replaced):
            path = w.inside(root, rel)
            if path.read_bytes() == plan[rel]:
                path.write_bytes(originals[rel])
        for path in reversed(created):
            if path.resolve().is_relative_to(root):
                path.unlink(missing_ok=True)
        for path in reversed(directories):
            try:
                path.rmdir()
            except OSError:
                pass
        raise
    return {"ok": True, "written": True, "integration": status, "next": "start", "application_work_authorized": False}
