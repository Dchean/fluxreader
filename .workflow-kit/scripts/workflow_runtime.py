"""Bounded, foreground workflow operations. Uses only the Python standard library."""
# Note: host-led execution and candidate continuation — ../.agents/notes/implemented/process/
# 2026-09-13-portable-project-workflow.md; copied projects use ../docs/workflow/design-note.md.
from __future__ import annotations

import argparse
import contextlib
import copy
import fnmatch
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import uuid
from datetime import datetime, timedelta
from pathlib import Path

import project_workflow as w
import workflow_bootstrap as boot

COMMANDS = ("bootstrap", "doctor", "start", "adopt", "onboard", "research", "cards", "checkpoint", "prepare", "begin", "finish",
            "verify", "review", "feedback", "dispatch", "review-cli", "run", "next", "recover", "extend", "batch", "accept")
HOSTS = {"current_agent", "codex-cli", "claude-code-cli", "custom"}
IGNORED = w.DENIED_PARTS | {".cache", "coverage", "htmlcov"}
RETRYABLE = {"test_failure", "review_failure"}


def fresh(prefix):
    return prefix + uuid.uuid4().hex


def read(root, name):
    return w.read_json(w.workflow_inside(root, name))


def save(root, name, value):
    """Atomic replacement of one owned record; the CLI holds the project lock."""
    path = w.workflow_inside(root, name)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    try:
        w.write_exclusive(temporary, value if isinstance(value, bytes) else w.encoded(value))
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def pid_alive(pid):
    if not isinstance(pid, int) or pid <= 0:
        return False
    if os.name == "nt":
        import ctypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.CloseHandle.argtypes = [ctypes.c_void_p]
        handle = kernel.OpenProcess(0x100000, False, pid)
        if not handle:
            return ctypes.get_last_error() == 5  # Unknown/access denied is not dead.
        try:
            return kernel.WaitForSingleObject(handle, 0) == 0x102
        finally:
            kernel.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


@contextlib.contextmanager
def project_lock(root):
    path = w.workflow_inside(root, "tasks/runtime/controller.lock")
    path.parent.mkdir(parents=True, exist_ok=True)
    token = fresh("lock-")
    try:
        w.write_exclusive(path, w.encoded({"pid": os.getpid(), "token": token, "started": w.iso(w.utc_now())}))
    except FileExistsError:
        raise ValueError("Project has a controller lock. Read start/next; use recover only after its process has exited.")
    try:
        yield
    finally:
        if path.is_file() and w.read_json(path).get("token") == token:
            path.unlink()


def must_check(root):
    result = w.check_project(root)
    if not result["ok"]:
        raise ValueError("; ".join(result["errors"]))
    return result


def task_read(root, ident):
    if not isinstance(ident, str) or not ident.startswith("TASK-"):
        raise ValueError("Expected TASK- identifier")
    return read(root, "tasks/items/" + w.relative_name(ident) + ".json")


def task_save(root, task):
    save(root, "tasks/items/" + task["id"] + ".json", task)
    write_card(root, task)
    refresh_project_state(root)


def policy_read(root):
    project = read(root, "tasks/PROJECT.json")
    return read(root, project["policy"])


def selected_snapshot(root, task):
    paths = task.get("snapshot_paths")
    if not paths:
        paths = read(root, task["input"]["manifest"])["roots"]
    return w.capture(root, paths, allow_missing=True)


def inventory(root):
    """Hash project changes, excluding run outputs, caches and secret/data-like files."""
    root = Path(root).resolve()
    result = {}
    pending = [root]
    while pending:
        directory = pending.pop()
        for path in sorted(directory.iterdir()):
            rel = path.relative_to(root).as_posix()
            logical = w.workflow_relative(root, rel)
            name = path.name.lower()
            if name in IGNORED or (logical is not None and logical.startswith(("tasks/evidence/", "tasks/runs/", "tasks/runtime/"))):
                continue
            if logical is not None and (logical.startswith("tasks/cards/") or logical in {"tasks/BACKLOG.md", "tasks/IN_PROGRESS.md", "tasks/DONE.md", "tasks/PROJECT_STATE.md"}):
                continue
            if name.startswith(".env") or name.endswith((".key", ".pem", ".p12", ".pfx", ".db", ".sqlite", ".sqlite3", ".pyc")) or name in {"auth.json", "credentials.json", ".coverage"}:
                continue
            w.inside(root, rel)
            if path.is_dir():
                pending.append(path)
            elif path.is_file():
                import hashlib
                if logical is not None and logical.startswith("tasks/items/") and path.suffix == ".json":
                    record = w.read_json(path)
                    record.pop("checkpoints", None)
                    result[rel] = w.digest(record)
                    continue
                with path.open("rb") as stream:
                    digest = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(block)
                result[rel] = digest.hexdigest()
            if len(result) > 50000:
                raise ValueError("Project scan exceeds 50,000 files; isolate the worktree or exclude generated dependencies first.")
    return result


def matches(path, patterns):
    return any(fnmatch.fnmatchcase(path, pattern) or path == pattern.rstrip("/**") for pattern in patterns)


def doctor(root):
    root = Path(root).resolve()
    entries = []
    if root.is_dir():
        entries = sorted(p.name for p in root.iterdir() if p.name not in {".env", ".claude", ".codex"})
    tools = {"python": sys.executable}
    for name in ("git", "node", "codex", "claude"):
        tools[name] = shutil.which(name) or shutil.which(name + ".ps1")
    git = None
    if root.is_dir() and tools["git"]:
        result = subprocess.run([tools["git"], "-C", str(root), "status", "--porcelain"],
                                capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=10)
        git = {"repository": result.returncode == 0, "uncommitted_entries": len(result.stdout.splitlines()) if result.returncode == 0 else None}
    return {"ok": True, "root": str(root), "exists": root.is_dir(), "entries": entries[:80],
            "tools": tools, "git": git, "model_calls_tested": False,
            "integration": boot.integration_status(root),
            "next": "Read INTAKE.md and ask up to three business questions; existing confirmed answers remain valid."}


def adopt(root, kind, name, write=False, full_docs=False):
    """Classic layout compatibility. New integrations should use bootstrap."""
    root = Path(root).resolve()
    if w.workflow_inside(root, "tasks/PROJECT.json").exists():
        status = boot.integration_status(root)
        if not status["connected"]:
            raise ValueError("Existing records are not a verified workflow-kit installation. Use bootstrap to inspect isolated integration or migration; adoption cannot reset them.")
        raise ValueError("Project records already exist. Use start/next; adoption cannot reset them.")
    plan = w.initialization_plan(root, kind, name, allow_existing=True, full_docs=full_docs)
    preserved, appended, originals = [], [], {}
    legacy_sources = [rel for rel in boot.LEGACY_NAMES if w.inside(root, rel).is_file()]
    for rel in list(plan):
        path = w.inside(root, rel)
        if not path.exists():
            continue
        if not path.is_file():
            raise ValueError("Existing directory conflicts with generated file: " + rel)
        before = path.read_bytes()
        if rel == "WORKFLOW.md" or rel.startswith("tasks/"):
            raise ValueError("Existing workflow record/view needs an explicit migration: " + rel)
        if rel in {"AGENTS.md", "CLAUDE.md"}:
            originals[rel] = before
            plan[rel] = boot.prepend_entry(before, "WORKFLOW.md", "scripts/project_workflow.py")
            appended.append(rel)
        elif rel.endswith(".md") and not rel.startswith("docs/workflow/"):
            preserved.append(rel)
            del plan[rel]
        elif before == plan[rel] and not rel.startswith("tasks/"):
            preserved.append(rel)
            del plan[rel]
        else:
            raise ValueError("Existing state/tool conflict needs an explicit migration: " + rel)
    plan[w.CLASSIC_BINDING] = w.encoded(boot.classic_binding(plan, "adopt: preserve existing project", legacy_sources))
    result = {"ok": True, "written": False, "root": str(root), "files": sorted(plan),
              "preserved": preserved, "prepended": appended, "application_work_authorized": False}
    if not write:
        return result
    if not root.parent.is_dir():
        raise ValueError("Target parent must exist")
    created = []
    try:
        for rel, data in plan.items():
            path = w.inside(root, rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            if rel in originals:
                if path.read_bytes() != originals[rel]:
                    raise ValueError("File changed since adoption preview: " + rel)
                save(root, rel, data)
            else:
                w.write_exclusive(path, data)
                created.append(path)
    except Exception:
        for rel, data in originals.items():
            save(root, rel, data)
        for path in reversed(created):
            if path.resolve().is_relative_to(root):
                path.unlink()
        raise
    result["written"] = True
    return result


def onboard(root, answers, source):
    """Record the owner's confirmed brief and authority, never infer approval from a timeout."""
    policy = policy_read(root)
    if policy["approval"]["status"] != "pending":
        raise ValueError("Onboarding already approved; edit through a new explicit decision, not re-onboarding.")
    for key in ("goal", "audience", "acceptance", "non_goals"):
        if key not in answers or (key != "non_goals" and not answers[key]):
            raise ValueError("Confirmed brief is missing " + key)
    if not all(isinstance(answers[key], str) and answers[key].strip() for key in ("goal", "audience")):
        raise ValueError("Goal and audience must be nonempty text")
    if not isinstance(answers["non_goals"], list) or not all(isinstance(x, str) for x in answers["non_goals"]):
        raise ValueError("non_goals must be a list")
    if not source or not isinstance(answers["acceptance"], list) or not all(isinstance(x, str) and x for x in answers["acceptance"]):
        raise ValueError("An approval source and concrete acceptance list are required")
    execution = answers.get("execution", {})
    for role in ("coder", "reviewer"):
        if execution.get(role) not in HOSTS:
            raise ValueError("Select a supported " + role + " harness")
    mode = execution.get("review_mode")
    if mode not in {"self_review_allowed", "independent_required"}:
        raise ValueError("Explicit review mode required")
    authority = answers.get("authority", {})
    if type(authority.get("code")) is not bool:
        raise ValueError("Explicit code authority required")
    finance = answers.get("financial", {})
    if finance.get("mode") not in {"none", "limit"}:
        raise ValueError("Explicit financial mode none/limit required; unknown is not unlimited")
    _, binding = boot.binding_record(root)
    legacy_sources = binding.get("legacy_sources", []) if binding else []
    if legacy_sources:
        review = answers.get("legacy_review", {})
        if not isinstance(review, dict):
            raise ValueError("legacy_review must record how inherited rules and workflow conflicts were handled")
        reviewed = review.get("reviewed_paths", [])
        if not isinstance(reviewed, list) or not all(isinstance(name, str) for name in reviewed) or not set(legacy_sources) <= set(reviewed):
            raise ValueError("Review the existing workflow/rule sources before onboarding; record legacy_review.reviewed_paths")
        constraints = review.get("preserved_constraints")
        if (not isinstance(constraints, list) or not constraints or not all(isinstance(value, str) and value.strip() for value in constraints)
                or not all(isinstance(review.get(key), str) and review[key].strip() for key in ("task_disposition", "workflow_resolution"))):
            raise ValueError("legacy_review must explain preserved constraints, old task disposition and current role/workflow resolution")
        if (not isinstance(review.get("unresolved_conflicts"), list)
                or not all(isinstance(value, str) for value in review["unresolved_conflicts"])
                or (review["unresolved_conflicts"] and authority.get("code"))):
            raise ValueError("Resolve inherited rule conflicts before authorizing application code")
    project = read(root, "tasks/PROJECT.json")
    if project["kind"] == "refactor" and not answers.get("compatibility"):
        raise ValueError("Refactor onboarding needs confirmed compatibility constraints")
    ui_mode = answers.get("ui", {}).get("mode", "none")
    if ui_mode not in {"none", "existing", "preview_first"}:
        raise ValueError("Choose UI mode none, existing or preview_first from the actual project needs")
    policy["ui"] = {"mode": ui_mode}
    decision = {"id": fresh("DEC-"), "issuer": "owner", "status": "accepted",
                "recorded_at_utc": w.iso(w.utc_now()), "statement": answers["goal"],
                "scope": ["workflow"], "source": source, "confirmed_brief": copy.deepcopy(answers)}
    decisions = read(root, "tasks/DECISIONS.json")
    decisions["decisions"].append(decision)
    policy["approval"] = {"status": "approved", "decision_ids": [decision["id"]]}
    policy["roles"] = {role: {"agent": role, "harness": execution.get(role, "current_agent")}
                       for role in ("manager", "coder", "reviewer")}
    policy["review"]["mode"] = mode
    policy["role_fusion"]["mode"] = "single-agent-multi-role" if execution["coder"] == execution["reviewer"] == "current_agent" else "manager-worker"
    policy["capabilities"].update(answers.get("capabilities", {}))
    for key, value in authority.items():
        if key not in policy["authority"]:
            raise ValueError("Unknown authority field: " + key)
        policy["authority"][key] = value
    for key in ("max_repair_rounds", "max_task_wall_minutes", "max_tasks_per_batch", "max_concurrent_workers", "batch_rollover"):
        if key in answers.get("budget", {}):
            policy["budget"][key] = answers["budget"][key]
    policy["budget"]["financial"] = {"mode": finance["mode"], "amount_usd": finance.get("amount_usd"), "scope": "batch"}
    if "recovery" in answers:
        policy["recovery"] = {**w.recovery_policy(policy), **answers["recovery"]}
    for key in ("worker_argv", "reviewer_argv", "model", "allow_non_git"):
        if key in execution:
            policy["execution"][key] = execution[key]
    project["stage"] = "discovery"
    updates = {"tasks/DECISIONS.json": decisions, project["policy"]: policy,
               "tasks/PROJECT.json": project, "tasks/BRIEF.json": {"schema_version": 1, **answers, "approval_source": source}}
    previous = {name: w.workflow_inside(root, name).read_bytes() if w.workflow_inside(root, name).is_file() else None for name in updates}
    try:
        for name, value in updates.items():
            save(root, name, value)
        must_check(root)
    except Exception:
        for name, value in previous.items():
            if value is None:
                w.workflow_inside(root, name).unlink(missing_ok=True)
            else:
                save(root, name, value)
        raise
    refresh_project_state(root)
    return {"ok": True, "decision_id": decision["id"], "next": "Prepare the first accepted vertical slice and its real verification commands."}


def prepare(root, specification):
    must_check(root)
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved":
        raise ValueError("Complete confirmed onboarding first")
    references_path = w.workflow_inside(root, "tasks/REFERENCES.json")
    if not references_path.is_file() or read(root, "tasks/REFERENCES.json").get("status") not in {"searched", "offline", "not_needed"}:
        raise ValueError("Record reference research first (including a reason for offline/not_needed).")
    task = read(root, "tasks/templates/TASK.json")
    for key in ("id", "title", "objective", "acceptance", "requirement_refs", "reference_ids", "decision_refs", "gates", "dependencies", "non_goals", "kind", "risk", "retry_safe", "ui_change", "ui_contract_ref", "ui_checks"):
        if key in specification:
            task[key] = copy.deepcopy(specification[key])
    require_ui_ready(root, task)
    if task["kind"] == "ui_preview":
        task["ui_change"] = True
    ident = task["id"]
    if ident == "TASK-000" or not ident.startswith("TASK-") or not task["title"]:
        raise ValueError("A named, non-placeholder TASK- id is required")
    w.relative_name(ident)
    if w.workflow_inside(root, "tasks/items/" + ident + ".json").exists():
        raise ValueError("Task already exists; keep its identity/history and use next")
    paths = specification.get("snapshot_paths")
    if not isinstance(paths, list) or not paths:
        raise ValueError("Explicit snapshot_paths required (absent new files are supported)")
    task["snapshot_paths"] = paths
    task["scope"]["allowed_paths"] = copy.deepcopy(specification.get("allowed_paths", paths))
    task["scope"]["protected_paths"] += specification.get("protected_paths", [])
    task["risk"] = specification.get("risk", {"level": "low", "decision_ids": []})
    if task.get("ui_change"):
        contract = task.get("ui_contract_ref")
        if not contract or not w.inside(root, contract).is_file():
            raise ValueError("Prepare a concrete UI contract before a UI task (ui_contract_ref)")
        if not task.get("ui_checks"):
            raise ValueError("UI tasks need explicit ui_checks, including relevant opened/focus/error states")
        paths = sorted(set(paths) | {contract})
        if task["kind"] != "ui_preview":
            task["scope"]["protected_paths"].append(contract)
        elif not matches(contract, task["scope"]["allowed_paths"]):
            task["scope"]["allowed_paths"].append(contract)
    project = read(root, "tasks/PROJECT.json")
    if project.get("ui_preview_task") and task["kind"] != "ui_preview":
        approved_contract = task_read(root, project["ui_preview_task"]).get("ui_contract_ref")
        if approved_contract:
            task["scope"]["protected_paths"].append(approved_contract)
    # A later slice may extend the same files. Carry earlier regression gates
    # and snapshot roots forward; the old PASS remains historical evidence.
    for dependency in task["dependencies"]:
        prior = task_read(root, dependency)
        if prior["status"] not in {"verified", "done"}:
            raise ValueError("Prepare this task after its dependency is verified: " + dependency)
        if prior["evidence"].get("continued_by"):
            raise ValueError("Depend on the latest continuation: " + prior["evidence"]["continued_by"])
        task.setdefault("continuation_of", {})[dependency] = prior["evidence"]["candidate_digest"]
        paths = sorted(set(paths) | set(read(root, prior["evidence"]["candidate_manifest"])["roots"]))
        existing_commands = {w.gate_command(gate) for gate in task["gates"] if gate.get("required")}
        for prior_gate in prior["gates"]:
            if prior_gate.get("required") and w.gate_command(prior_gate) not in existing_commands:
                inherited = copy.deepcopy(prior_gate)
                inherited["id"] = dependency + "-" + inherited["id"]
                task["gates"].append(inherited)
                existing_commands.add(w.gate_command(inherited))
    task["snapshot_paths"] = paths
    references = read(root, "tasks/REFERENCES.json")
    known_references = {entry["id"] for entry in references.get("candidates", [])}
    if any(ident not in known_references for ident in task.get("reference_ids", [])):
        raise ValueError("Task references an unknown research candidate")
    for reference in task.get("decision_refs", []):
        if not w.inside(root, reference.split("#")[0]).is_file():
            raise ValueError("Missing design decision reference: " + reference)
    for gate in task["gates"]:
        if not isinstance(gate, dict) or not isinstance(gate.get("id"), str):
            raise ValueError("Each gate requires an id and concrete argv")
        w.relative_name(gate["id"])
        if gate.get("required") and (not isinstance(gate.get("args"), list) or not all(isinstance(x, str) for x in gate["args"])):
            raise ValueError("Gate arguments must be a string array")
    project = read(root, "tasks/PROJECT.json")
    batch_id = project.get("current_batch") or fresh("BATCH-")
    batch_path = "tasks/batches/" + batch_id + ".json"
    if w.workflow_inside(root, batch_path).exists():
        batch = read(root, batch_path)
        if len(batch["task_ids"]) >= policy["budget"]["max_tasks_per_batch"]:
            rollover = new_batch(root)
            batch_id = rollover["batch_id"]
            batch_path = "tasks/batches/" + batch_id + ".json"
            batch = read(root, batch_path)
            project = read(root, "tasks/PROJECT.json")
    else:
        batch = {"schema_version": 1, "id": batch_id, "status": "active", "task_ids": [],
                 "approval_decision_ids": policy["approval"]["decision_ids"], "created_at_utc": w.iso(w.utc_now())}
    if batch["status"] != "active":
        raise ValueError("Current batch is closed")
    batch["task_ids"].append(ident)
    snapshot = w.capture(root, paths, allow_missing=True)
    manifest = w.workflow_name(root, "tasks/evidence/" + fresh(ident + "-input-") + ".json")
    task.update(status="ready", batch_id=batch_id)
    task["input"].update(manifest=manifest, digest=snapshot["digest"])
    task["input"]["definition_digest"] = w.task_definition(task)
    project.update(stage="delivery", current_batch=batch_id)
    if task["kind"] == "ui_preview":
        project["ui_preview_task"] = ident
    updates = {manifest: snapshot, "tasks/items/" + ident + ".json": task, batch_path: batch, "tasks/PROJECT.json": project}
    previous = {name: w.workflow_inside(root, name).read_bytes() if w.workflow_inside(root, name).is_file() else None for name in updates}
    try:
        for name, value in updates.items():
            save(root, name, value)
        must_check(root)
    except Exception:
        for name, value in previous.items():
            if value is None:
                w.workflow_inside(root, name).unlink(missing_ok=True)
            else:
                save(root, name, value)
        raise
    write_card(root, task)
    refresh_project_state(root)
    return {"ok": True, "task_id": ident, "batch_id": batch_id, "card": w.workflow_name(root, "tasks/cards/") + ident + ".md", "next": "begin (host agent) or dispatch (configured CLI)"}


def require_ui_ready(root, task):
    if task.get("kind") in {"ui_preview", "documentation", "baseline"}:
        return
    errors = []
    tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
    if errors:
        raise ValueError("; ".join(errors))
    if not w.ui_preview_accepted(root, policy_read(root), read(root, "tasks/PROJECT.json"), tasks):
        raise ValueError("Accept the UI preview before production implementation; prepare a ui_preview task first")


def record_research(root, research):
    status = research.get("status")
    if status not in {"searched", "offline", "not_needed"} or not research.get("reason"):
        raise ValueError("Research needs searched/offline/not_needed and an honest reason or conclusion")
    entries = research.get("candidates", [])
    if status == "searched" and not entries:
        raise ValueError("Searched research requires at least one actually inspected candidate")
    identifiers = set()
    for entry in entries:
        for key in ("id", "title", "url", "inspected_at_utc", "fit", "limitations", "reuse", "license", "evidence"):
            if not entry.get(key):
                raise ValueError("Research candidate missing " + key)
        if entry["id"] in identifiers or not entry["url"].startswith(("https://", "http://", "project:")):
            raise ValueError("Research ids must be unique and sources must be web URLs or project: paths")
        if entry["url"].startswith("project:") and not w.inside(root, entry["url"][8:]).exists():
            raise ValueError("Local reference does not exist")
        identifiers.add(entry["id"])
        if entry["reuse"] not in {"reference_only", "dependency", "adapt_code", "rejected"}:
            raise ValueError("Choose reference_only, dependency, adapt_code or rejected")
        if entry["reuse"] in {"dependency", "adapt_code"} and (entry["license"] in {"unknown", "unverified"} or not entry.get("revision")):
            raise ValueError("Direct reuse needs an inspected license and pinned revision/version")
    old = w.workflow_inside(root, "tasks/REFERENCES.json")
    if old.is_file():
        previous = read(root, "tasks/REFERENCES.json")
        old_ids = {entry["id"] for entry in previous.get("candidates", [])}
        if old_ids - identifiers:
            raise ValueError("Keep old candidate ids and mark them rejected; task references must remain resolvable")
        save(root, "tasks/evidence/" + fresh("research-history-") + ".json", previous)
    value = {"schema_version": 1, **research, "recorded_at_utc": w.iso(w.utc_now())}
    save(root, "tasks/REFERENCES.json", value)
    lines = [w.BOARD_MARKER, "# 参考方案调研", "", "状态：" + status, "", research["reason"], ""]
    for entry in entries:
        lines += ["## " + entry["id"] + " · " + entry["title"], "", "- 来源：" + entry["url"],
                  "- 采用方式：" + entry["reuse"], "- 适合之处：" + entry["fit"],
                  "- 限制：" + entry["limitations"], "- 许可证：" + entry["license"],
                  "- 固定版本：" + str(entry.get("revision") or "仅参考；未引入代码"),
                  "- 查验依据：" + entry["evidence"], ""]
    save(root, "tasks/RESEARCH.md", "\n".join(lines).encode("utf-8"))
    return {"ok": True, "status": status, "candidates": len(entries), "report": w.workflow_name(root, "tasks/RESEARCH.md")}


def write_card(root, task):
    path = w.workflow_name(root, "tasks/cards/" + task["id"] + ".md")
    old = w.workflow_inside(root, path)
    if old.is_file() and not old.read_text(encoding="utf-8-sig").startswith(w.BOARD_MARKER):
        raise ValueError("Refusing to overwrite a handwritten task card: " + path)
    budget = task.get("budget", {})
    progress = task.get("checkpoints", [])
    lines = [w.BOARD_MARKER, "# " + task["id"] + " · " + str(task.get("title")), "",
             "**状态**：" + str(task.get("status")), "", "**目标**：" + str(task.get("objective")), "",
             "**依赖**：" + (", ".join(task.get("dependencies", [])) or "无"),
             "**参考方案**：" + (", ".join(task.get("reference_ids", [])) or "见 ../RESEARCH.md"),
             "**界面约定**：" + str(task.get("ui_contract_ref") or "不涉及界面"),
             "**界面检查**：" + (", ".join(task.get("ui_checks", [])) or "不适用"),
             "**修改范围**：" + ", ".join(task.get("scope", {}).get("allowed_paths", [])), "",
             "## 验收标准", ""] + ["- " + item for item in task.get("acceptance", [])]
    lines += ["", "## 执行与恢复", "", "- 首次开始：" + str(budget.get("started_at_utc")),
              "- 原截止时间：" + str(budget.get("deadline_at_utc")),
              "- 当前截止时间：" + str(w.task_deadline(task)),
              "- 已用修复轮：" + str(budget.get("repair_rounds_used", 0)),
              "- 阻塞：" + ("；".join(task.get("blockers", [])) or "无"),
              "- 下一步：" + (progress[-1]["next_action"] if progress else "执行 start/next 获取可继续的动作"),
              "", "## 最近检查点", ""]
    for point in progress[-8:]:
        lines += ["- " + point["at_utc"] + "：" + point["note"] + "；下一步：" + point["next_action"]]
    lines += ["", "## 原始证据", "", "[唯一状态记录](../items/" + task["id"] + ".json)", ""]
    for ident in task.get("run_ids", []):
        lines.append("- [" + ident + "](../runs/" + ident + ".json)")
    lines += ["", "卡片是自动生成的视图。Agent 修改任务记录、执行命令或保存检查点后重新生成；不手工把状态改成通过。", ""]
    save(root, path, "\n".join(lines).encode("utf-8"))


def refresh_views(root):
    errors = []
    tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
    if errors:
        return {"ok": False, "cards": 0, "errors": errors}
    for task in tasks.values():
        write_card(root, task)
    for name, data in w.board_bytes(tasks).items():
        path = w.workflow_inside(root, name)
        if path.is_file() and not path.read_text(encoding="utf-8-sig").startswith(w.BOARD_MARKER):
            raise ValueError("Refusing to overwrite handwritten board: " + name)
        save(root, name, data)
    refresh_project_state(root, tasks)
    return {"ok": not errors, "cards": len(tasks), "errors": errors}


def refresh_project_state(root, tasks=None):
    path = w.workflow_name(root, "tasks/PROJECT_STATE.md")
    old = w.workflow_inside(root, path)
    if old.is_file() and not old.read_text(encoding="utf-8-sig").startswith(w.BOARD_MARKER):
        # Older releases produced a blank, unmarked template. Only that exact
        # template can be replaced automatically; handwritten facts are retained.
        legacy = Path(__file__).resolve().parent.parent / "assets/project/tasks/PROJECT_STATE.md.template"
        if not legacy.is_file() or old.read_bytes() != legacy.read_bytes():
            raise ValueError("Keep the handwritten PROJECT_STATE; move its facts to the task records before regenerating it")
    errors = []
    if tasks is None:
        tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
    if errors:
        raise ValueError("; ".join(errors))
    save(root, path, w.project_state_bytes(read(root, "tasks/PROJECT.json"), tasks, w.workflow_name(root, "scripts/project_workflow.py")))


def checkpoint(root, ident, note, next_action):
    if not note or not next_action:
        raise ValueError("Checkpoint needs completed/observed facts and a concrete next action")
    task = task_read(root, ident)
    task.setdefault("checkpoints", []).append({"at_utc": w.iso(w.utc_now()), "note": note, "next_action": next_action})
    task_save(root, task)
    return {"ok": True, "task_id": ident, "card": w.workflow_name(root, "tasks/cards/") + ident + ".md"}


def remaining_seconds(task):
    deadline = w.task_deadline(task)
    if not deadline:
        raise ValueError("Task has not started")
    seconds = (datetime.fromisoformat(deadline.replace("Z", "+00:00")) - w.utc_now()).total_seconds()
    if seconds <= 0:
        raise ValueError("Original task deadline exhausted; preserve the task and request a scoped budget decision")
    return seconds


def start_run(root, task, kind, context, harness):
    if not context:
        raise ValueError("Actual execution context id required")
    if any(read(root, "tasks/runs/" + ident + ".json")["outcome"] == "running" for ident in task["run_ids"]):
        raise ValueError("An earlier run is still running; inspect/recover it before starting another")
    policy = policy_read(root)
    errors = []
    runs = w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)
    if errors or any(run["outcome"] == "running" for run in runs.values()):
        raise ValueError("An earlier project run is still active; finish/recover it before starting another")
    if task["budget"]["started_at_utc"] is None:
        now = w.utc_now()
        task["budget"]["started_at_utc"] = w.iso(now)
        task["budget"]["deadline_at_utc"] = w.iso(now + timedelta(minutes=policy["budget"]["max_task_wall_minutes"]))
    remaining_seconds(task)
    if kind == "repair":
        if task["budget"]["repair_rounds_used"] >= w.repair_limit(task, policy):
            raise ValueError("Repair budget exhausted")
        task["budget"]["repair_rounds_used"] += 1
    run = read(root, "tasks/templates/RUN.json")
    run.update(id=fresh("RUN-"), task_id=task["id"], kind=kind, started_at_utc=w.iso(w.utc_now()),
               outcome="running", input_digest=task["input"]["digest"])
    run["executor"] = {"harness": harness, "model": policy.get("execution", {}).get("model"), "context_id": context}
    task["run_ids"].append(run["id"])
    task["blockers"] = []
    task.pop("failure_kind", None)
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    task_save(root, task)
    return run


def begin(root, ident, context, harness="current_agent"):
    task = task_read(root, ident)
    require_ui_ready(root, task)
    if task["status"] not in {"ready", "blocked"}:
        raise ValueError("Begin requires a ready task or a recorded recoverable blocker")
    if task["status"] == "blocked" and task.get("failure_kind") not in RETRYABLE | {"network", "interrupted", "environment", "budget"}:
        raise ValueError("Resolve this blocker explicitly before resuming")
    must_check(root)
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved" or read(root, "tasks/PROJECT.json")["stage"] in {"intake", "paused", "complete"}:
        raise ValueError("Project authorization/stage does not permit execution")
    permission = {"documentation": "documentation", "baseline": "baseline", "ci": "ci"}.get(task["kind"], "code")
    if policy["authority"].get(permission) is not True:
        raise ValueError("Task execution authority is not enabled")
    if task["input"]["definition_digest"] != w.task_definition(task):
        raise ValueError("Task definition changed; inspect the original task instead of rehashing it")
    if task["status"] == "blocked":
        remaining_seconds(task)
    kind = "repair" if task.get("failure_kind") in RETRYABLE else "implementation"
    run = start_run(root, task, kind, context, harness)
    task["status"] = "running"
    project = read(root, "tasks/PROJECT.json")
    project["current_task"] = ident
    save(root, "tasks/PROJECT.json", project)
    task_save(root, task)
    for dependency in task.get("continuation_of", {}):
        prior = task_read(root, dependency)
        prior["evidence"]["continued_by"] = ident
        task_save(root, prior)
    checkpoint(root, ident, "开始执行，保留原任务身份和截止时间", "完成当前修改后调用 finish，再运行 verify")
    scope_manifest = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-scope.json")
    save(root, scope_manifest, {"files": inventory(root)})
    run["scope_manifest"] = scope_manifest
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    return {"ok": True, "run_id": run["id"], "task_id": ident, "deadline_at_utc": w.task_deadline(task)}


def block(root, task, run, reason, kind, exit_code=None):
    # A failed/interrupted writer can still have changed files. Inspect that
    # attempt before another run captures a new baseline and hides the delta.
    if run.get("scope_manifest") and kind != "scope":
        try:
            before = read(root, run["scope_manifest"])["files"]
            after = inventory(root)
            changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
            save(root, "tasks/evidence/" + run["id"] + "-changes.json", {"changed_files": changed})
            prohibited = [name for name in changed if run["kind"] == "review" or matches(name, task["scope"]["protected_paths"])
                          or not matches(name, task["scope"]["allowed_paths"])]
            if prohibited:
                reason += "; out-of-scope changes: " + ", ".join(prohibited)
                kind = "scope"
        except (ValueError, OSError, KeyError, TypeError) as error:
            reason += "; could not inspect interrupted changes: " + str(error)
            kind = "scope"
    run.update(outcome="blocked", finished_at_utc=w.iso(w.utc_now()), exit_code=exit_code, failure_kind=kind, failure_reason=reason)
    task.update(status="blocked", blockers=[reason], failure_kind=kind, resume_phase=run["kind"])
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    task_save(root, task)
    checkpoint(root, task["id"], reason, "先核对已有文件及原始日志，再处理 " + kind + "；不要新建任务或重置预算")
    if kind == "network":
        # Freeze the complete partial result after bookkeeping. A later retry
        # must observe exactly this state, including policy and task definition.
        manifest = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-recovery.json")
        save(root, manifest, {"files": inventory(root)})
        run["recovery_manifest"] = manifest
        save(root, "tasks/runs/" + run["id"] + ".json", run)
    return {"ok": False, "task_id": task["id"], "run_id": run["id"], "failure_kind": kind, "error": reason}


def validate_worker_result(value, task, run):
    required = {"task_id", "run_id", "status", "summary", "changed_files", "validation_requests", "requested_actions", "unresolved_items", "blocked_reason"}
    if not isinstance(value, dict) or set(value) != required:
        raise ValueError("Worker result must match the complete worker-result contract")
    if value["task_id"] != task["id"] or value["run_id"] != run["id"]:
        raise ValueError("Worker result belongs to a different task/run")
    if value["status"] not in {"ready_for_verification", "action_requested", "blocked"} or not isinstance(value["summary"], str):
        raise ValueError("Invalid worker status/summary")
    for key in ("changed_files", "validation_requests", "requested_actions", "unresolved_items"):
        if not isinstance(value[key], list) or not all(isinstance(item, str) for item in value[key]):
            raise ValueError("Worker " + key + " must be an array of strings")
    if value["blocked_reason"] is not None and not isinstance(value["blocked_reason"], str):
        raise ValueError("Invalid blocked_reason")


def finish(root, run_id, result):
    run = read(root, "tasks/runs/" + w.relative_name(run_id) + ".json")
    task = task_read(root, run["task_id"])
    if run["outcome"] != "running" or run["kind"] not in {"implementation", "repair"}:
        raise ValueError("Only an active implementation run can finish")
    result_path = w.workflow_name(root, "tasks/evidence/" + run_id + "-worker-result.json")
    save(root, result_path, result)
    run["raw_output_refs"].append(result_path)
    try:
        validate_worker_result(result, task, run)
        before = read(root, run["scope_manifest"])["files"]
        after = inventory(root)
        changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
        save(root, "tasks/evidence/" + run_id + "-changes.json", {"changed_files": changed})
        prohibited = [name for name in changed if matches(name, task["scope"]["protected_paths"]) or not matches(name, task["scope"]["allowed_paths"])]
        if prohibited:
            return block(root, task, run, "Out-of-scope changes: " + ", ".join(prohibited), "scope")
        if set(changed) != set(result["changed_files"]):
            return block(root, task, run, "Worker changed_files does not match the observed project diff", "protocol")
        if result["status"] != "ready_for_verification" or result["requested_actions"] or result["unresolved_items"] or result["blocked_reason"]:
            return block(root, task, run, result["blocked_reason"] or "Worker requests manager action; inspect the result", "action_required")
        remaining_seconds(task)
        snapshot = selected_snapshot(root, task)
        manifest = w.workflow_name(root, "tasks/evidence/" + run_id + "-candidate.json")
        save(root, manifest, snapshot)
        run.update(outcome="completed", finished_at_utc=w.iso(w.utc_now()), exit_code=0, candidate_digest=snapshot["digest"])
        task["evidence"].update(candidate_manifest=manifest, candidate_digest=snapshot["digest"], verification_run=None, review_run=None)
        task.update(status="verifying", blockers=[])
        save(root, "tasks/runs/" + run_id + ".json", run)
        task_save(root, task)
        checkpoint(root, task["id"], "编码结果已记录，差异范围已核对", "运行 verify；代码完成尚未等于验收通过")
        return {"ok": True, "task_id": task["id"], "candidate_digest": snapshot["digest"], "next": "verify"}
    except (ValueError, OSError, KeyError, TypeError) as error:
        return block(root, task, run, str(error), "budget" if "deadline exhausted" in str(error) else "protocol")


def resolve_program(program):
    if program == "{python}":
        return [sys.executable]
    found = shutil.which(program) or (program if Path(program).is_file() else None)
    if not found:
        found = shutil.which(program + ".ps1")
    if not found:
        raise ValueError("Executable is not available: " + program)
    path = Path(found).resolve()
    if path.suffix.lower() not in {".cmd", ".bat", ".ps1"}:
        return [str(path)]
    base = path.parent
    if path.stem == "claude":
        native = base / "node_modules/@anthropic-ai/claude-code/bin/claude.exe"
        if native.is_file():
            return [str(native)]
    scripts = {"codex": "node_modules/@openai/codex/bin/codex.js",
               "claude": "node_modules/@anthropic-ai/claude-code/cli.js",
               "npm": "node_modules/npm/bin/npm-cli.js",
               "npx": "node_modules/npm/bin/npx-cli.js"}
    script = base / scripts.get(path.stem, "__unsupported__")
    node = shutil.which("node")
    if script.is_file() and node:
        return [node, str(script)]
    raise ValueError("Use a native executable or node + script argv instead of a shell wrapper: " + str(path))


def terminate_tree(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        killer = Path(os.environ.get("SystemRoot", "C:/Windows")) / "System32/taskkill.exe"
        subprocess.run([str(killer), "/PID", str(process.pid), "/T", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
    else:
        os.killpg(process.pid, signal.SIGKILL)
    process.wait(timeout=10)


def run_process(root, run, argv, cwd, timeout, stdin_text=None, product_test=False, label="process"):
    if not argv or not all(isinstance(arg, str) and "\0" not in arg for arg in argv):
        raise ValueError("Executable argv must be an array of strings")
    workdir = Path(root).resolve() if cwd == "." else w.inside(root, cwd)
    if not workdir.is_dir():
        raise ValueError("Command working directory is missing: " + cwd)
    out = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-" + label + ".stdout.txt")
    err = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-" + label + ".stderr.txt")
    w.inside(root, out).parent.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    # The task protocol and captured logs are UTF-8, including Python workers
    # on Windows where redirected text streams otherwise use a legacy codepage.
    environment.update(PYTHONUTF8="1", PYTHONIOENCODING="utf-8")
    if product_test:
        environment = {key: value for key, value in environment.items()
                       if not key.upper().startswith(("OPENAI_", "ANTHROPIC_", "CODEX_API_"))
                       and not key.upper().endswith(("_API_KEY", "_ACCESS_TOKEN", "_AUTH_TOKEN"))}
        environment["PYTHONDONTWRITEBYTECODE"] = "1"
    options = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {"start_new_session": True}
    started = w.iso(w.utc_now())
    timed_out = cancelled = False
    with w.inside(root, out).open("xb") as stdout, w.inside(root, err).open("xb") as stderr:
        process = subprocess.Popen(argv, cwd=workdir, env=environment, stdin=subprocess.PIPE if stdin_text is not None else subprocess.DEVNULL,
                                   stdout=stdout, stderr=stderr, shell=False, **options)
        run.update(pid=process.pid, controller_pid=os.getpid())
        run["raw_output_refs"] += [out, err]
        run.setdefault("commands", []).append({"argv": argv, "cwd": cwd, "started_at_utc": started, "stdout": out, "stderr": err})
        save(root, "tasks/runs/" + run["id"] + ".json", run)
        try:
            process.communicate(None if stdin_text is None else stdin_text.encode("utf-8"), timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            terminate_tree(process)
        except KeyboardInterrupt:
            cancelled = True
            terminate_tree(process)
        finally:
            if process.poll() is None:
                terminate_tree(process)
    metadata = {"argv": argv, "cwd": cwd, "started_at_utc": started, "finished_at_utc": w.iso(w.utc_now()),
                "exit_code": process.returncode, "timed_out": timed_out, "cancelled": cancelled, "stdout": out, "stderr": err}
    run["commands"][-1].update(metadata)
    run.update(timed_out=timed_out, exit_code=process.returncode)
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    return metadata


def verify(root, ident):
    task = task_read(root, ident)
    require_ui_ready(root, task)
    if task["evidence"].get("continued_by"):
        raise ValueError("Verify the latest continuation instead: " + task["evidence"]["continued_by"])
    if task["status"] not in {"verifying", "review", "verified", "blocked"}:
        raise ValueError("Finish implementation before verification")
    if task["status"] == "blocked" and task.get("failure_kind") not in RETRYABLE | {"environment", "interrupted", "network", "budget"}:
        raise ValueError("Resolve the recorded blocker before verification")
    remaining_seconds(task)
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved" or read(root, "tasks/PROJECT.json")["stage"] in {"intake", "paused", "complete"}:
        raise ValueError("Project authorization/stage does not permit verification")
    if task["input"]["definition_digest"] != w.task_definition(task):
        raise ValueError("Task/gates changed after preparation; do not rehash to conceal the change")
    snapshot = selected_snapshot(root, task)
    run = start_run(root, task, "verification", fresh("tool-"), "local_process")
    manifest = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-candidate.json")
    save(root, manifest, snapshot)
    task["evidence"].update(candidate_manifest=manifest, candidate_digest=snapshot["digest"], verification_run=run["id"], review_run=None)
    task["status"] = "verifying"
    task_save(root, task)
    run["candidate_digest"] = snapshot["digest"]
    try:
        for gate in task["gates"]:
            if not gate["required"]:
                run["checks"].append({"gate_id": gate["id"], "status": "NOT_APPLICABLE", "reason": gate["reason"]})
                continue
            argv = resolve_program(gate["program"]) + gate["args"]
            timeout = min(remaining_seconds(task), float(gate.get("timeout_seconds", 300)))
            if timeout <= 0:
                raise ValueError("Gate timeout must be positive")
            outcome = run_process(root, run, argv, gate["cwd"], timeout, product_test=True, label=gate["id"])
            run["checks"].append({"gate_id": gate["id"], "status": "PASS" if outcome["exit_code"] == 0 and not outcome["timed_out"] and not outcome["cancelled"] else "FAIL",
                                  "exit_code": outcome["exit_code"], "log": outcome["stdout"], "stderr": outcome["stderr"],
                                  "candidate_digest": snapshot["digest"], "command": outcome})
            save(root, "tasks/runs/" + run["id"] + ".json", run)
            if outcome["timed_out"] or outcome["cancelled"]:
                return block(root, task, run, "Verification timed out/cancelled; original logs and deadline retained", "interrupted", outcome["exit_code"])
            if outcome["exit_code"] != 0:
                category = classify_process_failure(root, outcome)
                if category == "network" and selected_snapshot(root, task)["digest"] != snapshot["digest"]:
                    category = "scope"
                return block(root, task, run, "Required gate failed: " + gate["id"],
                             category if category in {"network", "scope", "environment"} else "test_failure", outcome["exit_code"])
        if selected_snapshot(root, task)["digest"] != snapshot["digest"]:
            return block(root, task, run, "Verification changed the candidate; inspect and verify the resulting version", "scope")
        run.update(outcome="completed", finished_at_utc=w.iso(w.utc_now()), exit_code=0)
        task.update(status="review", blockers=[])
        save(root, "tasks/runs/" + run["id"] + ".json", run)
        task_save(root, task)
        checkpoint(root, ident, "预先定义的必需测试全部通过，日志已保存", "审查当前候选；独立审查使用没有参与编码的新上下文")
        return {"ok": True, "task_id": ident, "run_id": run["id"], "candidate_digest": snapshot["digest"], "next": "review"}
    except (ValueError, OSError, KeyError, TypeError) as error:
        return block(root, task, run, str(error), "environment")


def review(root, ident, report, context, mode="self_review", existing_run=None):
    task = task_read(root, ident)
    if task["status"] != "review":
        raise ValueError("Complete verification before review")
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved":
        raise ValueError("Project authorization does not permit review")
    if mode not in {"self_review", "independent"}:
        raise ValueError("Unknown review mode")
    if policy["review"]["mode"] == "independent_required" and mode != "independent":
        raise ValueError("Self review cannot replace required independent review")
    writers = {read(root, "tasks/runs/" + run_id + ".json")["executor"]["context_id"]
               for run_id in task["run_ids"] if read(root, "tasks/runs/" + run_id + ".json")["kind"] in {"implementation", "repair"}}
    if mode == "independent" and (not context or context in writers):
        raise ValueError("Independent reviewer must have a new, non-writer context")
    current = selected_snapshot(root, task)["digest"]
    if current != task["evidence"]["candidate_digest"] or report.get("candidate_digest") != current or report.get("task_id") != ident:
        raise ValueError("Review report is not for the current verified candidate")
    if report.get("verdict") not in {"PASS", "FAIL", "BLOCKED"} or not isinstance(report.get("findings"), list) or not report.get("summary"):
        raise ValueError("Review needs verdict, findings and a substantive summary")
    ui_digest = w.ui_review_digest(root, task, report) if report["verdict"] == "PASS" and not report["findings"] else None
    run = existing_run or start_run(root, task, "review", context, "current_agent")
    path = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-review.json")
    save(root, path, report)
    run.update(candidate_digest=current)
    run["raw_output_refs"].append(path)
    run["review"] = {"mode": mode, "verdict": report["verdict"], "report": path, "candidate_digest": current}
    if ui_digest:
        run["review"]["ui_evidence_digest"] = ui_digest
    if report["verdict"] != "PASS" or report["findings"]:
        return block(root, task, run, "Review requires changes; inspect the findings", "review_failure")
    run.update(outcome="completed", finished_at_utc=w.iso(w.utc_now()), exit_code=0)
    task["evidence"]["review_run"] = run["id"]
    task.update(status="verified", blockers=[])
    project = read(root, "tasks/PROJECT.json")
    if project.get("current_task") == ident:
        project["current_task"] = None
        save(root, "tasks/PROJECT.json", project)
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    task_save(root, task)
    check = w.check_project(root)
    if not check["ok"]:
        task.update(status="blocked", blockers=check["errors"], failure_kind="evidence")
        task_save(root, task)
        return check
    checkpoint(root, ident, "当前候选的测试与审查通过", "继续已授权任务；所属功能完成后请用户验收")
    return {"ok": True, "task_id": ident, "status": "verified", "next": "next"}


def record_ui_feedback(root, ident, note, source):
    """A rejected preview continues in its original task, with the same budget."""
    must_check(root)
    task = task_read(root, ident)
    if not note.strip() or not source.strip():
        raise ValueError("Record the actual UI feedback and its conversation source")
    if task["kind"] != "ui_preview" or task["status"] != "verified" or task["evidence"].get("continued_by"):
        raise ValueError("Feedback requires the current, verified but not yet accepted UI preview")
    errors = []
    runs = w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)
    if errors or any(run["outcome"] == "running" for run in runs.values()):
        raise ValueError("Wait for the active project run before recording preview feedback")
    task.setdefault("feedback", []).append({"at_utc": w.iso(w.utc_now()), "note": note, "source": source})
    task.update(status="blocked", failure_kind="review_failure", blockers=["UI preview feedback: " + note])
    task_save(root, task)
    checkpoint(root, ident, "用户反馈：" + note, "在原预览任务与剩余预算内调整，重新验证后展示；不要重建任务清零")
    return {"ok": True, "task_id": ident, "next": "run", "status": "blocked"}


def cli_arguments(root, policy, harness, review_only, result_path):
    schema_name = "review-result.schema.json" if review_only else "worker-result.schema.json"
    schema = w.workflow_inside(root, "docs/workflow/contracts/" + schema_name)
    if harness == "custom":
        argv = policy.get("execution", {}).get("reviewer_argv" if review_only else "worker_argv")
        if not isinstance(argv, list) or not argv or not all(isinstance(arg, str) for arg in argv):
            raise ValueError("Custom harness needs approved executable argv")
        return resolve_program(argv[0]) + argv[1:]
    program = "codex" if harness == "codex-cli" else "claude"
    launcher = resolve_program(program)
    help_args = ["exec", "--help"] if harness == "codex-cli" else ["--help"]
    help_result = subprocess.run(launcher + help_args, capture_output=True, text=True, encoding="utf-8",
                                 errors="replace", timeout=20, cwd=root)
    help_text = help_result.stdout
    required = ["--sandbox", "--output-schema", "--output-last-message"] if harness == "codex-cli" else ["--json-schema", "--permission-mode", "--strict-mcp-config", "--restricted"]
    if help_result.returncode != 0 or any(flag not in help_text for flag in required):
        raise ValueError("Installed CLI does not provide the required noninteractive controls; inspect its help before adapting")
    if harness == "codex-cli":
        argv = launcher + ["exec", "--sandbox", "read-only" if review_only else "workspace-write",
                           "-c", 'approval_policy="never"', "--json", "--output-schema", str(schema),
                           "--output-last-message", str(w.inside(root, result_path))]
        git = shutil.which("git")
        in_git = bool(git and subprocess.run([git, "-C", str(root), "rev-parse", "--is-inside-work-tree"],
                                            capture_output=True, timeout=10).returncode == 0)
        if not in_git:
            if not policy.get("execution", {}).get("allow_non_git"):
                raise ValueError("Codex target has no Git repository; explicitly approve file-snapshot mode or initialize Git")
            argv.append("--skip-git-repo-check")
        argv.append("-")
    else:
        permitted = "Read,Glob,Grep" if review_only else "Read,Edit,Write,Glob,Grep"
        argv = launcher + ["--print", "--output-format", "json", "--permission-mode", "dontAsk",
                           "--restricted", "--tools", permitted, "--allowedTools", permitted,
                           "--strict-mcp-config", "--mcp-config", str(w.workflow_inside(root, "docs/workflow/contracts/empty-mcp.json")),
                           "--json-schema", schema.read_text(encoding="utf-8")]
    model = policy.get("execution", {}).get("model")
    if model:
        argv += ["--model", model]
    return argv


def check_call_budget(root, task, policy, harness, argv):
    finance = policy["budget"]["financial"]
    if finance["mode"] == "none":
        return argv
    if harness != "claude-code-cli":
        raise ValueError("This harness has no supported hard per-call cost cap; use the provider's approved budget controls before changing financial policy")
    batch = read(root, "tasks/batches/" + task["batch_id"] + ".json")
    errors = []
    runs = w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)
    relevant = [run for run in runs.values() if run["task_id"] in batch["task_ids"] and run["executor"]["harness"] in {"codex-cli", "claude-code-cli", "custom"} and run["outcome"] != "running"]
    if any(run.get("estimated_cost_usd") is None for run in relevant):
        raise ValueError("Previous CLI usage is unknown; do not count it as free or repeat a paid call")
    remaining = finance["amount_usd"] - sum(run["estimated_cost_usd"] for run in relevant)
    if remaining <= 0:
        raise ValueError("Recorded batch cost budget exhausted")
    return argv + ["--max-budget-usd", str(remaining)]


def packet(root, task, run, review_only=False):
    brief = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
    research = read(root, "tasks/REFERENCES.json") if w.workflow_inside(root, "tasks/REFERENCES.json").is_file() else {}
    prompt_name = "REVIEW.md" if review_only else "WORKER.md"
    instruction = w.workflow_inside(root, "docs/workflow/prompts/" + prompt_name).read_text(encoding="utf-8")
    context = {"task_id": task["id"], "run_id": run["id"], "task": task, "brief": brief,
               "reference_candidates": [item for item in research.get("candidates", []) if item["id"] in task.get("reference_ids", [])],
               "read_first": ["AGENTS.md", "WORKFLOW-KIT.md" if w.inside(root, w.ISOLATED_BINDING).is_file() else "WORKFLOW.md"], "candidate_digest": task["evidence"].get("candidate_digest"),
               "deadline_at_utc": w.task_deadline(task)}
    if review_only:
        context["verification"] = read(root, "tasks/runs/" + task["evidence"]["verification_run"] + ".json")
    if task.get("ui_change"):
        context["read_first"] += [task["ui_contract_ref"], w.workflow_name(root, "docs/workflow/FRONTEND.md")]
        context["ui_evidence_directory"] = w.workflow_name(root, "tasks/evidence/") + task["id"] + "-ui/" + str(task["evidence"].get("verification_run") or "pending") + "/"
    return json.dumps({"instructions": instruction, "task_packet": context}, ensure_ascii=False, indent=2)


def cli_result(root, harness, outcome, result_path, run):
    if harness == "codex-cli":
        value = read(root, result_path)
        for line in w.inside(root, outcome["stdout"]).read_text(encoding="utf-8-sig", errors="replace").splitlines():
            event = json.loads(line)
            if event.get("type") in {"error", "turn.failed"}:
                raise ValueError("Codex reported an error event; inspect its raw JSONL")
        return value
    outer = read(root, outcome["stdout"])
    if harness == "claude-code-cli":
        if outer.get("is_error") or outer.get("permission_denials"):
            raise ValueError("Claude reported an error or permission denial; preserve its result")
        cost = outer.get("total_cost_usd")
        import math
        if type(cost) in {int, float} and math.isfinite(cost) and cost >= 0:
            run["estimated_cost_usd"] = cost
        save(root, "tasks/runs/" + run["id"] + ".json", run)
        value = outer.get("structured_output")
        if not isinstance(value, dict):
            raise ValueError("Claude returned no structured_output")
        return value
    return outer


def dispatch(root, ident, review_only=False):
    policy = policy_read(root)
    harness = policy["roles"]["reviewer" if review_only else "coder"]["harness"]
    if harness not in HOSTS - {"current_agent"}:
        raise ValueError("This project uses a host Agent here; use begin/finish or review instead")
    task = task_read(root, ident)
    context = fresh("cli-context-")
    if review_only:
        must_check(root)
        if task["status"] != "review":
            raise ValueError("Verification must pass before CLI review")
        run = start_run(root, task, "review", context, harness)
    else:
        opened = begin(root, ident, context, harness)
        run = read(root, "tasks/runs/" + opened["run_id"] + ".json")
        task = task_read(root, ident)
    output_path = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-final.json")
    before = inventory(root) if review_only else None
    if review_only:
        run["scope_manifest"] = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-scope.json")
        save(root, run["scope_manifest"], {"files": before})
        save(root, "tasks/runs/" + run["id"] + ".json", run)
    try:
        argv = check_call_budget(root, task, policy, harness, cli_arguments(root, policy, harness, review_only, output_path))
        prompt = packet(root, task, run, review_only)
        save(root, "tasks/evidence/" + run["id"] + "-input.txt", prompt.encode("utf-8"))
        timeout = min(remaining_seconds(task), w.recovery_policy(policy)["max_cli_call_seconds"])
        outcome = run_process(root, run, argv, ".", timeout, prompt, label="reviewer" if review_only else "worker")
        if outcome["timed_out"] or outcome["cancelled"]:
            return block(root, task, run, "CLI interrupted; inspect partial changes before resuming", "interrupted", outcome["exit_code"])
        if outcome["exit_code"] != 0:
            category = classify_process_failure(root, outcome)
            return block(root, task, run, "CLI failed; inspect stdout/stderr and existing files before retrying",
                         "environment" if category == "unknown" else category, outcome["exit_code"])
        value = cli_result(root, harness, outcome, output_path, run)
        if review_only:
            if inventory(root) != before:
                return block(root, task, run, "Reviewer modified project files", "scope")
            return review(root, ident, value, context, "independent", existing_run=run)
        return finish(root, run["id"], value)
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        return block(root, task, run, str(error), "environment")


def classify_process_failure(root, outcome):
    """Only recognized transport failures qualify; auth/configuration never do."""
    chunks = []
    for key in ("stdout", "stderr"):
        path = w.inside(root, outcome[key])
        with path.open("rb") as stream:
            stream.seek(max(0, path.stat().st_size - 6000))
            chunks.append(stream.read().decode("utf-8", errors="replace").lower())
    message = "\n".join(chunks)
    permanent = ("permission denied", "invalid_api_key", "incorrect api key",
                 "insufficient_quota", "quota exceeded", "billing", "unauthorized", "forbidden",
                 "authentication", "http 401", "http 403", "certificate_verify_failed",
                 "self signed certificate", "modulenotfounderror", "command not found")
    if any(word in message for word in permanent) or re.search(r'"permission_denials"\s*:\s*\[(?!\s*\])', message):
        return "environment"
    transient = ("econnreset", "econnrefused", "enotfound", "eai_again", "etimedout", "connecttimeout",
                 "readtimeout", "connection reset", "connection refused", "connection timed out", "request timed out",
                 "temporary failure in name resolution", "rate_limit_error")
    if any(word in message for word in transient) or re.search(r'(?:http|status(?:_code)?)"?\s*[=:]?\s*(?:429|502|503|504)\b', message):
        return "network"
    return "unknown"


def run_phase(kind):
    return "writer" if kind in {"implementation", "repair"} else kind


def network_retry_plan(root, task):
    """Derive the streak and due time from original RUNs, including old sessions."""
    denied = {"allowed": False, "next": "inspect_blocker"}
    if not task.get("retry_safe", False) or task.get("risk", {}).get("level") == "high":
        return {**denied, "reason": "Automatic replay requires an explicitly repeatable, non-high-risk task"}
    if task.get("failure_kind") != "network":
        return {**denied, "reason": "Only a recognized transient network failure can retry automatically"}
    phase = run_phase(task.get("resume_phase"))
    if phase not in {"writer", "verification", "review"}:
        return {**denied, "reason": "Inspect this interrupted operation before repeating it"}
    streak = []
    for ident in reversed(task["run_ids"]):
        run = read(root, "tasks/runs/" + ident + ".json")
        if run_phase(run["kind"]) != phase:
            continue
        if run.get("failure_kind") != "network" or run["outcome"] not in {"blocked", "failed"}:
            break
        streak.append(run)
    delays = w.recovery_policy(policy_read(root))["network_backoff_seconds"]
    if not streak or len(streak) > len(delays):
        return {**denied, "reason": "Automatic network retry budget exhausted; inspect connectivity and preserve the same task",
                "automatic_retries_used": max(0, len(streak) - 1)}
    failed = streak[0]
    errors = []
    runs = w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)
    if errors or any(item["outcome"] == "running" for item in runs.values()) or pid_alive(failed.get("pid")):
        return {**denied, "next": "inspect_active_run", "reason": "An existing process/run may still be active"}
    if task["input"]["definition_digest"] != w.task_definition(task):
        return {**denied, "reason": "Task definition changed after the failure"}
    if not failed.get("recovery_manifest") or read(root, failed["recovery_manifest"])["files"] != inventory(root):
        return {**denied, "reason": "The recorded partial result is missing or changed; inspect it before replay"}
    finished = datetime.fromisoformat(failed["finished_at_utc"].replace("Z", "+00:00"))
    due = finished + timedelta(seconds=delays[len(streak) - 1])
    delay = max(0, (due - w.utc_now()).total_seconds())
    try:
        if delay >= remaining_seconds(task):
            raise ValueError("Not enough original task time remains for retry backoff")
    except ValueError as error:
        return {**denied, "next": "budget_decision", "reason": str(error)}
    return {"allowed": True, "phase": phase, "attempt": len(streak), "max_retries": len(delays),
            "failed_run": failed["id"], "retry_at_utc": w.iso(due), "wait_seconds": delay}


def retry_network(root, task):
    must_check(root)
    plan = network_retry_plan(root, task)
    if not plan["allowed"]:
        return {"ok": False, "task_id": task["id"], "failure_kind": "network", "retry_stopped": True,
                "next": plan["next"], "error": plan["reason"]}
    checkpoint(root, task["id"], f"短时网络错误，自动重试 {plan['attempt']}/{plan['max_retries']}；已有文件和进程已核对",
               "不早于 " + plan["retry_at_utc"] + " 继续原任务；保留原时钟、失败日志与修复额度")
    try:
        if plan["wait_seconds"]:
            time.sleep(plan["wait_seconds"])
    except KeyboardInterrupt:
        checkpoint(root, task["id"], "自动退避等待已取消，没有发起新调用", "下次从原任务核对后继续")
        return {"ok": False, "task_id": task["id"], "failure_kind": "interrupted", "next": "inspect_blocker"}
    task = task_read(root, task["id"])
    rechecked = network_retry_plan(root, task)
    if not rechecked["allowed"]:
        return {"ok": False, "task_id": task["id"], "failure_kind": "network", "retry_stopped": True,
                "next": rechecked["next"], "error": rechecked["reason"]}
    policy = policy_read(root)
    if plan["phase"] == "writer":
        if policy["roles"]["coder"]["harness"] == "current_agent":
            return {"ok": True, "task_id": task["id"], "next": "begin", "requires_host_agent": True, "retry": plan}
        return dispatch(root, task["id"])
    if plan["phase"] == "review" and selected_snapshot(root, task)["digest"] == task["evidence"]["candidate_digest"]:
        task.update(status="review", blockers=[])
        task.pop("failure_kind", None)
        task_save(root, task)
        if policy["roles"]["reviewer"]["harness"] == "current_agent":
            return {"ok": True, "task_id": task["id"], "next": "review", "requires_host_agent": True, "retry": plan}
        return dispatch(root, task["id"], review_only=True)
    return verify(root, task["id"])


def no_progress(root, task):
    failures = []
    for ident in reversed(task["run_ids"]):
        run = read(root, "tasks/runs/" + ident + ".json")
        if run.get("failure_kind") in RETRYABLE:
            failures.append(run)
            if len(failures) == 2:
                break
    if len(failures) < 2:
        return False
    signatures = [(run.get("failure_kind"), run.get("failure_reason"), run.get("candidate_digest")) for run in failures]
    return (all(signatures[0]) and signatures[0] == signatures[1]
            and selected_snapshot(root, task)["digest"] == signatures[0][2])


def run_task(root, ident):
    """Continue one task within its original budget. The host coordinates the batch."""
    while True:
        task = task_read(root, ident)
        if task["status"] in {"verified", "done"}:
            must_check(root)
            return {"ok": True, "task_id": ident, "status": task["status"]}
        if task["status"] == "blocked" and task.get("failure_kind") == "network":
            result = retry_network(root, task)
            if result.get("retry_stopped"):
                return result
        elif task["status"] in {"ready", "blocked"}:
            if task["status"] == "blocked" and task.get("failure_kind") not in RETRYABLE:
                return {"ok": False, "task_id": ident, "failure_kind": task.get("failure_kind"), "error": "Inspect and resolve the interruption/environment before continuing"}
            if task["status"] == "blocked" and no_progress(root, task):
                checkpoint(root, ident, "连续两次相同失败且候选未变化，停止重复修复", "主 Agent 分析根因、记录不同方案，再在剩余预算内 begin；不要重复 run")
                return {"ok": False, "task_id": ident, "failure_kind": task["failure_kind"], "next": "replan", "no_progress": True}
            if policy_read(root)["roles"]["coder"]["harness"] == "current_agent":
                return {"ok": True, "task_id": ident, "next": "begin", "requires_host_agent": True}
            result = dispatch(root, ident)
        elif task["status"] == "verifying":
            result = verify(root, ident)
        elif task["status"] == "review":
            if policy_read(root)["roles"]["reviewer"]["harness"] == "current_agent":
                return {"ok": True, "task_id": ident, "next": "review", "requires_host_agent": True}
            result = dispatch(root, ident, review_only=True)
        else:
            return {"ok": False, "task_id": ident, "error": "An existing run needs inspection or recovery"}
        if result.get("requires_host_agent"):
            return result
        if not result.get("ok") and result.get("failure_kind") not in RETRYABLE | {"network"}:
            return result
        if not result.get("ok") and result.get("failure_kind") in RETRYABLE:
            task = task_read(root, ident)
            if task["budget"]["repair_rounds_used"] >= w.repair_limit(task, policy_read(root)):
                return {**result, "budget_exhausted": True}


def next_action(root):
    integration = boot.integration_status(root)
    if not integration["connected"] and integration["status"] != "not_connected":
        return {"ok": False, "next": "bootstrap" if integration["status"] == "foreign_workflow" else "repair_integration",
                "integration": integration, "errors": integration["errors"],
                "instruction": "尚未接入当前 workflow-kit；不要沿旧工作流继续，也不要清空旧任务。先完成可核对的接入。"}
    result = _next_action(root)
    result["integration"] = integration
    return result


def _next_action(root):
    root = Path(root).resolve()
    if not w.workflow_inside(root, "tasks/PROJECT.json").is_file():
        return {"ok": True, "stage": "intake", "next": "ask", "questions": [
            "你希望做什么，主要给谁使用？", "这是新项目，还是要改善现有项目？", "第一版做到什么程度就可以开始使用？"],
            "instruction": "先只读检查目录。确认范围后用 init（空目录）或 adopt（已有目录），由 Agent 维护配置。"}
    policy = policy_read(root)
    project = read(root, "tasks/PROJECT.json")
    errors = []
    tasks = w.records(root, "tasks/items", "TASK-", errors)
    runs = w.records(root, "tasks/runs", "RUN-", errors)
    if errors:
        return {"ok": False, "errors": errors, "next": "repair_records_or_refresh_stale_evidence"}
    lock = w.workflow_inside(root, "tasks/runtime/controller.lock")
    lock_info = read(root, "tasks/runtime/controller.lock") if lock.is_file() else None
    # Recovery information must remain visible even when the clock expired or
    # a controller died halfway through writing its records.
    for run in runs.values():
        if run["outcome"] == "running":
            return {"ok": True, "next": "inspect_active_run", "task_id": run["task_id"], "run_id": run["id"],
                    "pid": run.get("pid"), "process_alive": pid_alive(run.get("pid")), "controller": lock_info,
                    "card": w.workflow_name(root, "tasks/cards/") + run["task_id"] + ".md", "instruction": "先检查日志和现有文件；进程已退出且结果未记录时，用 recover 保留中断证据。"}
    if lock_info:
        alive = pid_alive(lock_info.get("pid"))
        return {"ok": True, "next": "inspect_controller" if alive else "recover_lock", "controller": lock_info,
                "instruction": "控制器仍存在时不要接管；确认其退出后可用 recover --source 清理孤立锁，保留原记录。"}
    for task in tasks.values():
        deadline = w.task_deadline(task)
        if deadline and task["status"] in w.ACTIVE:
            if datetime.fromisoformat(deadline.replace("Z", "+00:00")) <= w.utc_now():
                return {"ok": True, "next": "budget_decision", "task_id": task["id"], "deadline_at_utc": deadline,
                        "instruction": "原任务期限已耗尽。保留现场；取得延长预算的明确决定后用 extend，不重建任务。"}
    check = w.check_project(root)
    if not check["ok"]:
        return {**check, "next": "repair_records_or_refresh_stale_evidence", "instruction": "保持已有身份、记录和代码；不能重初始化清空问题。"}
    if project["stage"] in {"paused", "complete"}:
        return {"ok": True, "stage": project["stage"], "next": project["stage"]}
    if policy["approval"]["status"] != "approved":
        return {"ok": True, "stage": "intake", "next": "ask", "instruction": "读取已保存的 BRIEF 和用户决定，补充缺失的产品答案并确认执行摘要，再 onboard。"}
    if not w.workflow_inside(root, "tasks/REFERENCES.json").is_file():
        return {"ok": True, "stage": "research", "next": "research", "instruction": "检索少量类似项目/成熟组件；保存来源、适配性、许可证和采用方式。网络不可用时如实记录 offline 及恢复步骤。"}
    priorities = {"verifying": 0, "review": 1, "ready": 2, "blocked": 3}
    for task in sorted(tasks.values(), key=lambda item: (priorities.get(item["status"], 9), item["id"])):
        if task["status"] not in priorities:
            continue
        if any(tasks.get(dep, {}).get("status") not in {"verified", "done"} for dep in task["dependencies"]):
            continue
        action = {"ready": "begin" if policy["roles"]["coder"]["harness"] == "current_agent" else "run",
                  "verifying": "verify", "review": "review" if policy["roles"]["reviewer"]["harness"] == "current_agent" else "run",
                  "blocked": "inspect_blocker"}[task["status"]]
        if task["status"] == "blocked" and task.get("failure_kind") in RETRYABLE:
            action = "repair" if task["budget"]["repair_rounds_used"] < w.repair_limit(task, policy) else "budget_decision"
            if action == "repair" and no_progress(root, task):
                action = "replan"
        if task["status"] == "blocked" and task.get("failure_kind") == "network":
            retry = network_retry_plan(root, task)
            action = "run" if retry["allowed"] else retry["next"]
        if task["status"] == "blocked" and w.task_deadline(task):
            if datetime.fromisoformat(w.task_deadline(task).replace("Z", "+00:00")) <= w.utc_now():
                action = "budget_decision"
        return {"ok": True, "task_id": task["id"], "status": task["status"], "next": action,
                "card": w.workflow_name(root, "tasks/cards/") + task["id"] + ".md", "blockers": task["blockers"],
                "deadline_at_utc": w.task_deadline(task), "checkpoints": task.get("checkpoints", [])[-3:],
                "resume_with": "verify" if task.get("resume_phase") in {"verification", "review"} else "begin"}
    if not w.ui_preview_accepted(root, policy, project, tasks):
        preview = tasks.get(project.get("ui_preview_task"), {})
        if preview.get("status") == "verified":
            return {"ok": True, "next": "accept_ui_preview", "task_id": preview["id"],
                    "instruction": "展示可点击预览、控件展开状态和真实截图，请用户确认视觉及交互，再 accept；确认前不接入正式后端。"}
        if not preview or preview.get("status") == "done":
            return {"ok": True, "next": "prepare_ui_preview",
                    "instruction": "先做关键技术可行性检查和 UI 约定，再准备 ui_preview 任务；已确认约定变化时重新预览确认。复用计划沿用的组件，使用模拟数据。"}
    if any(task["status"] == "verified" for task in tasks.values()):
        return {"ok": True, "next": "accept_or_prepare_next_authorized_task", "instruction": "验证通过的任务保留证据；向用户展示可运行成果并按功能验收。"}
    if tasks and all(task["status"] in {"done", "cancelled"} for task in tasks.values()):
        return {"ok": True, "next": "prepare_or_finish_project", "tasks": len(tasks),
                "instruction": "已建立的任务已验收；核对 BRIEF 是否还有未拆分范围。全部交付后才记录项目完成。"}
    return {"ok": True, "next": "prepare", "instruction": "Agent 将已确认范围拆成可验证小任务；不要让用户手填任务 JSON。"}


def recover(root, run_id, source):
    if not source:
        raise ValueError("Record why recovery is justified after inspecting the process and files")
    lock = w.workflow_inside(root, "tasks/runtime/controller.lock")
    run = read(root, "tasks/runs/" + w.relative_name(run_id) + ".json") if run_id else None
    errors = []
    runs = w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)
    if errors:
        raise ValueError("; ".join(errors))
    if run is not None and run["outcome"] != "running":
        raise ValueError("Run is already closed; use next")
    if any(item["outcome"] == "running" and pid_alive(item.get("pid")) for item in runs.values()):
        raise ValueError("Recorded worker is still alive or inaccessible; inspect it before recovery")
    recovered_lock = False
    if lock.exists():
        state = read(root, "tasks/runtime/controller.lock")
        if pid_alive(state.get("pid")):
            raise ValueError("Recorded controller is still alive or inaccessible; do not take over")
        save(root, "tasks/evidence/" + fresh("recovered-lock-") + ".json", state)
        if read(root, "tasks/runtime/controller.lock") != state:
            raise ValueError("Controller lock changed while inspecting it; do not take over")
        lock.unlink()
        recovered_lock = True
    if run is None:
        if not recovered_lock:
            raise ValueError("No orphan controller lock found; specify the interrupted --run or use next")
        return {"ok": True, "recovered_lock": True, "next": "next"}
    with project_lock(root):
        task = task_read(root, run["task_id"])
        run["recovery"] = {"source": source, "observed_at_utc": w.iso(w.utc_now()),
                           "actual_process_finish_known": False, "note": "finished_at marks record closure, not a fabricated process completion"}
        result = block(root, task, run, "Interrupted run recovered: " + source, "interrupted")
        refresh_views(root)
        return {**result, "ok": True, "recovered": True, "next": "next"}


def extend_budget(root, ident, minutes, repair_rounds, source):
    """Add an auditable allowance; never replace the first clock or run history."""
    if not source or type(minutes) is not int or minutes <= 0 or type(repair_rounds) is not int or repair_rounds < 0:
        raise ValueError("A real owner decision, positive minutes and nonnegative repair rounds are required")
    task = task_read(root, ident)
    if task["status"] in {"done", "cancelled"} or not w.task_deadline(task):
        raise ValueError("Only a started, unfinished task can receive a budget extension")
    if any(read(root, "tasks/runs/" + run_id + ".json")["outcome"] == "running" for run_id in task["run_ids"]):
        raise ValueError("Inspect/recover the active run before extending its budget")
    now = w.utc_now()
    previous = w.task_deadline(task)
    previous_time = datetime.fromisoformat(previous.replace("Z", "+00:00"))
    decision_id = fresh("DEC-")
    extension = {"decision_id": decision_id, "recorded_at_utc": w.iso(now), "minutes": minutes,
                 "repair_rounds": repair_rounds, "previous_deadline_at_utc": previous,
                 "deadline_at_utc": w.iso(max(previous_time, now) + timedelta(minutes=minutes))}
    decisions = read(root, "tasks/DECISIONS.json")
    decisions["decisions"].append({"id": decision_id, "issuer": "owner", "status": "accepted", "source": source,
                                  "statement": "Owner extended this task's time/repair allowance; financial limits remain in force",
                                  "scope": [ident], "recorded_at_utc": w.iso(now), "budget_extension": extension})
    task["budget"].setdefault("extensions", []).append(extension)
    previous_decisions = read(root, "tasks/DECISIONS.json")
    previous_task = task_read(root, ident)
    try:
        save(root, "tasks/DECISIONS.json", decisions)
        task_save(root, task)
        must_check(root)
    except Exception:
        save(root, "tasks/DECISIONS.json", previous_decisions)
        task_save(root, previous_task)
        raise
    checkpoint(root, ident, "依据新决定追加预算；原始时钟与失败记录保留", "先核对已有成果，再按原任务范围继续")
    return {"ok": True, "task_id": ident, "decision_id": decision_id, "deadline_at_utc": w.task_deadline(task)}


def new_batch(root, source=None):
    must_check(root)
    project = read(root, "tasks/PROJECT.json")
    policy = policy_read(root)
    if not project.get("current_batch") or project["stage"] in {"intake", "paused", "complete"}:
        raise ValueError("No active delivery batch to continue")
    previous = read(root, "tasks/batches/" + project["current_batch"] + ".json")
    if not previous["task_ids"] or any(task_read(root, ident)["status"] not in {"verified", "done", "cancelled"} for ident in previous["task_ids"]):
        raise ValueError("Finish the current batch before opening another one")
    auto = policy["budget"].get("batch_rollover") == "allowed" and policy["budget"]["financial"]["mode"] == "none"
    if not source and not auto:
        raise ValueError("Batch budget reached; use batch --source with an actual continuation decision. Do not reset the existing batch.")
    approvals = policy["approval"]["decision_ids"]
    if source:
        decisions = read(root, "tasks/DECISIONS.json")
        decision_id = fresh("DEC-")
        decisions["decisions"].append({"id": decision_id, "issuer": "owner", "status": "accepted", "source": source,
                                      "statement": "Owner approved the next bounded batch under the existing brief and policy",
                                      "scope": ["workflow"], "recorded_at_utc": w.iso(w.utc_now())})
        save(root, "tasks/DECISIONS.json", decisions)
        approvals = [decision_id]
    ident = fresh("BATCH-")
    previous["status"] = "closed"
    save(root, "tasks/batches/" + previous["id"] + ".json", previous)
    save(root, "tasks/batches/" + ident + ".json", {"schema_version": 1, "id": ident, "status": "active",
         "task_ids": [], "approval_decision_ids": approvals, "created_at_utc": w.iso(w.utc_now())})
    project.update(current_batch=ident, current_task=None, stage="delivery")
    save(root, "tasks/PROJECT.json", project)
    refresh_project_state(root)
    return {"ok": True, "batch_id": ident, "previous_batch": previous["id"], "next": "prepare"}


def accept(root, identifiers, source, merge_ref, project_complete=False):
    must_check(root)
    if not source or not merge_ref:
        raise ValueError("Actual owner acceptance source and merge reference/not_applicable explanation required")
    tasks = [task_read(root, ident) for ident in identifiers]
    if not tasks or any(task["status"] not in ({"verified", "done"} if project_complete else {"verified"}) for task in tasks):
        raise ValueError("Only currently verified tasks can be accepted")
    for task in tasks:
        successor_id = task["evidence"].get("continued_by")
        if successor_id and successor_id not in identifiers and task_read(root, successor_id)["status"] != "done":
            raise ValueError("Accept the latest integrated continuation together with its earlier tasks: " + successor_id)
    if project_complete:
        errors = []
        all_tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
        if errors or any(task["id"] not in identifiers and task["status"] not in {"done", "cancelled"} for task in all_tasks.values()):
            raise ValueError("Project completion requires all recorded tasks to be accepted or cancelled")
    decisions = read(root, "tasks/DECISIONS.json")
    decision = {"id": fresh("DEC-"), "issuer": "owner", "status": "accepted", "source": source,
                "statement": "Owner accepted the complete project" if project_complete else "Owner accepted the listed candidate versions and their continuations", "scope": identifiers,
                "recorded_at_utc": w.iso(w.utc_now()),
                "candidates": {task["id"]: task["evidence"]["candidate_digest"] for task in tasks}}
    decisions["decisions"].append(decision)
    evidence = w.workflow_name(root, "tasks/evidence/" + decision["id"] + "-acceptance.json")
    save(root, evidence, decision)
    save(root, "tasks/DECISIONS.json", decisions)
    for task in tasks:
        task["evidence"].update(acceptance_ref=evidence, owner_decision_ids=[decision["id"]], merge_ref=merge_ref)
        task["status"] = "done"
        task_save(root, task)
    project = read(root, "tasks/PROJECT.json")
    if project.get("current_task") in identifiers:
        project["current_task"] = None
    if project_complete:
        project["stage"] = "complete"
    save(root, "tasks/PROJECT.json", project)
    must_check(root)
    refresh_views(root)
    return {"ok": True, "accepted": identifiers, "decision_id": decision["id"], "next": "next"}


def main(argv=None):
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in COMMANDS:
        item = sub.add_parser(command)
        item.add_argument("--root", default=".", required=command == "bootstrap")
        if command == "bootstrap":
            item.add_argument("--kind", choices=("new", "refactor"), default="refactor")
            item.add_argument("--name")
            item.add_argument("--host", default="current_agent")
            item.add_argument("--source")
            item.add_argument("--write", action="store_true")
            item.add_argument("--full-docs", action="store_true")
        if command == "adopt":
            item.add_argument("--kind", choices=("new", "refactor"), default="refactor")
            item.add_argument("--name", required=True)
            item.add_argument("--write", action="store_true")
            item.add_argument("--full-docs", action="store_true")
        if command in {"onboard", "research", "prepare", "finish", "review"}:
            item.add_argument("--file", required=True, help="Agent-prepared JSON file; users do not need to edit it")
        if command in {"onboard", "recover", "accept", "extend", "feedback"}:
            item.add_argument("--source", required=True, help="Reference to the actual user approval/recovery decision")
        if command == "batch":
            item.add_argument("--source", help="Actual owner decision, unless existing policy permits automatic rollover")
        if command in {"begin", "verify", "review", "feedback", "dispatch", "review-cli", "run", "checkpoint", "extend"}:
            item.add_argument("--task", required=True)
        if command in {"begin", "review"}:
            item.add_argument("--context", required=True)
        if command == "review":
            item.add_argument("--mode", choices=("self_review", "independent"), required=True)
        if command in {"finish", "recover"}:
            item.add_argument("--run", required=command == "finish")
        if command == "extend":
            item.add_argument("--minutes", type=int, required=True)
            item.add_argument("--repair-rounds", type=int, default=0)
        if command in {"checkpoint", "feedback"}:
            item.add_argument("--note", required=True)
        if command == "checkpoint":
            item.add_argument("--next-action", required=True)
        if command == "accept":
            item.add_argument("--tasks", nargs="+", required=True)
            item.add_argument("--merge-ref", required=True)
            item.add_argument("--project-complete", action="store_true", help="The source also confirms the whole brief is delivered")
    args = parser.parse_args(argv)
    root = Path(args.root).resolve()

    def execute():
        command = args.command
        if command == "bootstrap":
            return boot.bootstrap(root, args.kind, args.name, args.source, args.host, args.write, args.full_docs)
        if command == "doctor":
            return doctor(root)
        if command in {"start", "next"}:
            return next_action(root)
        if command == "cards":
            return refresh_views(root)
        if command == "adopt":
            return adopt(root, args.kind, args.name, args.write, args.full_docs)
        if command == "onboard":
            return onboard(root, w.read_json(args.file), args.source)
        if command == "research":
            return record_research(root, w.read_json(args.file))
        if command == "prepare":
            return prepare(root, w.read_json(args.file))
        if command == "begin":
            return begin(root, args.task, args.context)
        if command == "finish":
            return finish(root, args.run, w.read_json(args.file))
        if command == "verify":
            return verify(root, args.task)
        if command == "review":
            return review(root, args.task, w.read_json(args.file), args.context, args.mode)
        if command == "feedback":
            return record_ui_feedback(root, args.task, args.note, args.source)
        if command in {"dispatch", "review-cli"}:
            return dispatch(root, args.task, command == "review-cli")
        if command == "run":
            return run_task(root, args.task)
        if command == "checkpoint":
            return checkpoint(root, args.task, args.note, args.next_action)
        if command == "recover":
            return recover(root, args.run, args.source)
        if command == "extend":
            return extend_budget(root, args.task, args.minutes, args.repair_rounds, args.source)
        if command == "batch":
            return new_batch(root, args.source)
        return accept(root, args.tasks, args.source, args.merge_ref, args.project_complete)

    try:
        if args.command in {"bootstrap", "doctor", "start", "next", "adopt", "recover"}:
            result = execute()
        else:
            integration = boot.integration_status(root)
            if not integration["connected"]:
                raise ValueError("Workflow integration must be verified before changing records: " + integration["status"])
            if not w.workflow_inside(root, "tasks/PROJECT.json").is_file():
                raise ValueError("Initialize/adopt the project before mutating workflow records")
            with project_lock(root):
                result = execute()
                refresh_views(root)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0 if result.get("ok") else 1
    except (ValueError, OSError, KeyError, TypeError, AttributeError, subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "errors": [str(error)]}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
