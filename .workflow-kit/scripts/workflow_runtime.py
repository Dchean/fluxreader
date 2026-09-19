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
import workflow_intake as intake_flow
import workflow_progress as progress_view

COMMANDS = ("bootstrap", "doctor", "start", "progress", "review-packet", "adopt", "intake", "onboard", "research", "cards", "checkpoint", "prepare", "begin", "finish",
            "verify", "review", "feedback", "dispatch", "review-cli", "run", "next", "recover", "extend", "batch", "accept",
            "unblock", "cancel", "diff", "note", "recompute", "rebind", "resume")
HOSTS = intake_flow.HOSTS
IGNORED = w.DENIED_PARTS | {".cache", "coverage", "htmlcov", "target", "build", "dist", "out", ".next", ".nuxt", ".svelte-kit",
                            ".turbo", ".parcel-cache", ".gradle", ".idea", ".vscode", ".DS_Store", "thumbs.db", ".tox", ".nox", ".eggs"}
RETRYABLE = {"test_failure", "review_failure"}
# Blockers the tool cannot resolve by itself. unblock --source records the
# owner's disposition and returns the task to the phase it must repeat.
UNBLOCKABLE = {"scope", "protocol", "action_required", "evidence", "environment", "interrupted", "budget", "network"} | RETRYABLE
NOTE_KINDS = ("context", "decision", "todo", "lesson", "progress")


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


def must_check(root, task_ids=(), baseline=()):
    """Stop on global errors and on errors owned by the tasks being changed.

    Errors owned by other tasks, or already present before this operation
    (baseline), are reported through start/next instead of blocking work here.
    """
    result = w.check_project(root)
    blocking = w.relevant_errors(result["errors"], task_ids, baseline)
    if blocking:
        raise ValueError("; ".join(blocking))
    return result


def journal(root, event, detail, task_id=None):
    """Append one line to the project journal; never rewrite history."""
    path = w.workflow_inside(root, "notes/JOURNAL.md")
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file():
        path.write_bytes((w.BOARD_MARKER.replace("generated view; edit task JSON instead", "append-only journal; use checkpoint/note")
                          + "\n# 项目日志\n\n工具在每个关键事件后追加一行；Agent 用 note 追加上下文、决策、待办和教训。不要手工改写历史行。\n\n").encode("utf-8"))
    text = " ".join(str(detail or "").split())
    line = "- " + w.iso(w.utc_now()) + " · " + event + " · " + ((task_id + " · ") if task_id else "") + text + "\n"
    with path.open("ab") as stream:
        stream.write(line.encode("utf-8"))


def note(root, kind, text, task_id=None):
    """Agent-authored note in the journal: context, decision, todo, lesson or progress."""
    if kind not in NOTE_KINDS:
        raise ValueError("note kind must be one of " + ", ".join(NOTE_KINDS))
    if not text or not text.strip():
        raise ValueError("A note needs actual content")
    if task_id:
        task_read(root, task_id)
    journal(root, "note/" + kind, text, task_id)
    refresh_project_state(root)
    return {"ok": True, "kind": kind, "resume": w.workflow_name(root, "notes/RESUME.md")}


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


def gitignore_rules(directory):
    """Rules from one .gitignore: (negated, anchored, pattern, dir_only). Comments and escapes are skipped."""
    path = Path(directory) / ".gitignore"
    rules = []
    if not path.is_file():
        return rules
    for raw in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw.rstrip()
        if not line or line.lstrip().startswith("#") or "\\" in line:
            continue
        negated = line.startswith("!")
        if negated:
            line = line[1:]
        dir_only = line.endswith("/")
        line = line.strip("/") if not line.startswith("/") else line[1:].rstrip("/")
        anchored = raw.lstrip("!").startswith("/") or "/" in line
        if line:
            rules.append((negated, anchored, line, dir_only))
    return rules


def ignored_by_git(rel, name, is_dir, layers):
    """Evaluate .gitignore layers from the root down; the last matching rule wins."""
    verdict = False
    for base, rules in layers:
        local = rel if not base else rel[len(base) + 1:]
        for negated, anchored, pattern, dir_only in rules:
            if dir_only and not is_dir:
                continue
            if anchored:
                hit = fnmatch.fnmatchcase(local, pattern) or fnmatch.fnmatchcase(local, pattern + "/*")
            else:
                hit = fnmatch.fnmatchcase(name, pattern) or any(fnmatch.fnmatchcase(part, pattern) for part in local.split("/")[:-1])
            if hit:
                verdict = not negated
    return verdict


def inventory(root):
    """Hash project changes, excluding run outputs, caches, ignored paths and tool-owned records.

    Task records hash only their frozen definition, so status changes,
    checkpoints and blockers written by the tool never look like worker edits.
    """
    root = Path(root).resolve()
    result = {}
    pending = [(root, [("", gitignore_rules(root))])]
    while pending:
        directory, layers = pending.pop()
        for path in sorted(directory.iterdir()):
            rel = path.relative_to(root).as_posix()
            logical = w.workflow_relative(root, rel)
            name = path.name.lower()
            is_dir = path.is_dir()
            if name in IGNORED or ignored_by_git(rel, path.name, is_dir, layers):
                continue
            if logical is not None and (logical.startswith(("tasks/evidence/", "tasks/runs/")) or w.is_mutable_record(root, rel)):
                if not (logical.startswith("tasks/items/") and path.suffix == ".json"):
                    continue
            if w.secret_kind(path.name) or name.endswith(".pyc") or name == ".coverage":
                continue
            w.inside(root, rel)
            if is_dir:
                nested = gitignore_rules(path)
                pending.append((path, layers + [(rel, nested)] if nested else layers))
            elif path.is_file():
                import hashlib
                if logical is not None and logical.startswith("tasks/items/") and path.suffix == ".json":
                    try:
                        result[rel] = w.task_definition(w.read_json(path))
                    except (ValueError, OSError):
                        result[rel] = "unreadable-task-record"
                    continue
                with path.open("rb") as stream:
                    digest = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(block)
                result[rel] = digest.hexdigest()
            if len(result) > 50000:
                raise ValueError("Project scan exceeds 50,000 files; add generated directories to .gitignore or exclude dependencies first.")
    return result


def matches(path, patterns):
    """Glob match; a pattern without wildcards also covers everything under that directory."""
    for pattern in patterns:
        pattern = pattern.rstrip("/")
        if not pattern:
            continue
        if fnmatch.fnmatchcase(path, pattern) or path == pattern.rstrip("/**"):
            return True
        plain = pattern[:-3] if pattern.endswith("/**") else pattern
        if not any(char in plain for char in "*?[") and path.startswith(plain + "/"):
            return True
    return False


def observed_changes(root, run, since="task"):
    """Files that differ from a baseline: the task's cumulative baseline (default) or this run's own start."""
    manifest = run.get("baseline_manifest") if since == "task" else None
    before = read(root, manifest or run["scope_manifest"])["files"]
    after = inventory(root)
    return sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))


def classify_changes(task, changed):
    protected = [name for name in changed if matches(name, task["scope"]["protected_paths"])]
    outside = [name for name in changed if name not in protected and not matches(name, task["scope"]["allowed_paths"])]
    inside_scope = [name for name in changed if name not in protected and name not in outside]
    return {"in_scope": inside_scope, "protected": protected, "outside": outside}


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


def intake_answers(root, submitted=None):
    defaults = read(root, "tasks/templates/BRIEF.json")
    previous = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
    return intake_flow.resolve(defaults, previous, submitted or {}, read(root, "tasks/PROJECT.json")["kind"])


def intake(root, answers=None, write=False):
    """Audit or save an unfinished interview without approving application work."""
    if policy_read(root)["approval"]["status"] != "pending":
        raise ValueError("Onboarding already approved; retain its decisions and use an explicit scope/policy update")
    brief = intake_answers(root, answers)
    result = intake_flow.audit(brief, read(root, "tasks/PROJECT.json")["kind"])
    if write:
        save(root, "tasks/BRIEF.json", {"schema_version": 1, **brief})
    return {**result, "written": write, "application_work_authorized": False}


def onboard(root, answers, source):
    """Record the owner's confirmed brief and authority, never infer approval from a timeout."""
    policy = policy_read(root)
    if policy["approval"]["status"] != "pending":
        raise ValueError("Onboarding already approved; edit through a new explicit decision, not re-onboarding.")
    answers = intake_answers(root, answers)
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
    interview = intake_flow.audit(answers, project["kind"])
    if not interview["ready_for_onboard"]:
        raise ValueError("Intake incomplete or unconfirmed: " + ", ".join(interview["missing"]) + "; run intake and ask the missing questions before onboard")
    policy["ui"] = {"mode": ui_mode}
    decision = {"id": fresh("DEC-"), "issuer": "owner", "status": "accepted",
                "recorded_at_utc": w.iso(w.utc_now()), "statement": answers["goal"],
                "scope": ["workflow"], "source": source, "confirmed_brief": copy.deepcopy(answers)}
    decisions = read(root, "tasks/DECISIONS.json")
    decisions["decisions"].append(decision)
    policy["approval"] = {"status": "approved", "decision_ids": [decision["id"]]}
    policy["intake"] = {"version": 1, "decision_id": decision["id"]}
    policy["roles"] = {role: {"agent": role, "harness": execution.get(role, "current_agent")}
                       for role in ("manager", "coder", "reviewer")}
    policy["review"]["mode"] = mode
    policy["role_fusion"]["mode"] = ("host-agent-independent-review" if mode == "independent_required" else "single-agent-multi-role") if execution["coder"] == execution["reviewer"] == "current_agent" else "manager-worker"
    policy["capabilities"].update(answers.get("capabilities", {}))
    for key, value in authority.items():
        if key not in policy["authority"]:
            raise ValueError("Unknown authority field: " + key)
        policy["authority"][key] = value
    for key in ("max_repair_rounds", "max_task_wall_minutes", "max_tasks_per_batch", "max_concurrent_workers", "batch_rollover", "clock"):
        if key in answers.get("budget", {}):
            policy["budget"][key] = answers["budget"][key]
    if isinstance(answers.get("acceptance_policy"), dict):
        policy["acceptance"] = {**policy.get("acceptance", {}), **answers["acceptance_policy"]}
    policy["budget"]["financial"] = {"mode": finance["mode"], "amount_usd": finance.get("amount_usd"), "scope": "batch"}
    if "recovery" in answers:
        policy["recovery"] = {**w.recovery_policy(policy), **answers["recovery"]}
    for key in ("mode", "worker_argv", "reviewer_argv", "model", "allow_non_git"):
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
    journal(root, "onboard", "已确认需求与执行方式；决定 " + decision["id"] + "；目标：" + answers["goal"])
    refresh_project_state(root)
    return {"ok": True, "decision_id": decision["id"], "next": "Prepare the first accepted vertical slice and its real verification commands."}


def prepare(root, specification):
    ident_hint = specification.get("id") if isinstance(specification, dict) and isinstance(specification.get("id"), str) else None
    must_check(root, [ident_hint] if ident_hint else ())
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved":
        raise ValueError("Complete confirmed onboarding first")
    references_path = w.workflow_inside(root, "tasks/REFERENCES.json")
    if not references_path.is_file() or read(root, "tasks/REFERENCES.json").get("status") not in {"searched", "offline", "not_needed"}:
        raise ValueError("Record reference research first (including a reason for offline/not_needed).")
    problems = []
    task = read(root, "tasks/templates/TASK.json")
    for key in ("id", "title", "objective", "acceptance", "requirement_refs", "reference_ids", "decision_refs", "gates", "dependencies", "non_goals", "kind", "risk", "retry_safe", "ui_change", "ui_contract_ref", "ui_checks", "test_review"):
        if key in specification:
            task[key] = copy.deepcopy(specification[key])
    require_ui_ready(root, task)
    if task["kind"] == "ui_preview":
        task["ui_change"] = True
    ident = task["id"]
    if not isinstance(ident, str) or ident == "TASK-000" or not ident.startswith("TASK-") or not task["title"]:
        raise ValueError("A named, non-placeholder TASK- id and a title are required (id like TASK-001)")
    w.relative_name(ident)
    if w.workflow_inside(root, "tasks/items/" + ident + ".json").exists():
        raise ValueError("Task already exists; keep its identity/history and use next")
    paths = specification.get("snapshot_paths")
    if not isinstance(paths, list) or not paths or not all(isinstance(item, str) for item in paths):
        raise ValueError("Explicit snapshot_paths (list of project-relative files/directories) required; absent new files are supported")
    for name in paths:
        try:
            w.relative_name(name)
        except ValueError as error:
            problems.append("snapshot_paths: " + str(error))
    task["snapshot_paths"] = paths
    task["scope"]["allowed_paths"] = copy.deepcopy(specification.get("allowed_paths", paths))
    task["scope"]["protected_paths"] += specification.get("protected_paths", [])
    task["risk"] = specification.get("risk", {"level": "low", "decision_ids": []})
    if not isinstance(task["risk"], dict) or task["risk"].get("level") not in {"low", "medium", "high"}:
        problems.append("risk must be {\"level\": low|medium|high, \"decision_ids\": [...]}")
    for key in ("acceptance", "requirement_refs", "dependencies", "reference_ids", "decision_refs", "non_goals"):
        if not isinstance(task.get(key), list) or not all(isinstance(item, str) for item in task[key]):
            problems.append(key + " must be a list of strings")
    if not task.get("objective") or not task.get("acceptance") or not task.get("requirement_refs"):
        problems.append("objective, acceptance and requirement_refs must be nonempty (requirement_refs are ids from BRIEF.requirements)")
    if task.get("ui_change"):
        contract = task.get("ui_contract_ref")
        if not contract or not w.inside(root, contract).is_file():
            problems.append("ui_contract_ref must point to an existing UI contract file (for example docs/UI.md)")
        if not task.get("ui_checks"):
            problems.append("ui_checks must list the component/interaction states to verify, for example select.open, select.keyboard")
        if contract and w.inside(root, contract).is_file():
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
    # Retain earlier checks unless an explicit applicability review replaces
    # them. Original results and snapshot roots always remain historical evidence.
    dependencies = {}
    for dependency in task["dependencies"] if isinstance(task["dependencies"], list) else []:
        try:
            prior = task_read(root, dependency)
        except (ValueError, OSError) as error:
            problems.append("dependencies: " + dependency + " does not exist (" + str(error) + ")")
            continue
        dependencies[dependency] = prior
        if prior["status"] not in {"verified", "done"}:
            problems.append("dependencies: " + dependency + " is " + prior["status"] + "; prepare this task after it is verified")
            continue
        if prior["evidence"].get("continued_by"):
            problems.append("dependencies: " + dependency + " was continued by " + prior["evidence"]["continued_by"] + "; depend on that latest continuation instead")
            continue
        task.setdefault("continuation_of", {})[dependency] = prior["evidence"]["candidate_digest"]
        paths = sorted(set(paths) | set(read(root, prior["evidence"]["candidate_manifest"])["roots"]))
        existing_commands = {w.gate_command(gate) for gate in task["gates"] if isinstance(gate, dict) and gate.get("required")}
        replaced = w.replaced_gates(task, dependency)
        for prior_gate in prior["gates"]:
            if prior_gate.get("id") in replaced:
                continue
            if prior_gate.get("required") and w.gate_command(prior_gate) not in existing_commands:
                inherited = copy.deepcopy(prior_gate)
                inherited["id"] = dependency + "-" + inherited["id"]
                task["gates"].append(inherited)
                existing_commands.add(w.gate_command(inherited))
    decisions = {item["id"]: item for item in read(root, "tasks/DECISIONS.json")["decisions"]}
    problems += w.test_review_errors(root, project, task, dependencies, decisions)
    if isinstance(task.get("test_review"), dict) and isinstance(task["test_review"].get("baseline"), dict):
        evidence_ref = task["test_review"]["baseline"].get("evidence_ref")
        if isinstance(evidence_ref, str) and evidence_ref:
            paths = sorted(set(paths) | {evidence_ref})
            task["scope"]["protected_paths"].append(evidence_ref)
    task["snapshot_paths"] = paths
    references = read(root, "tasks/REFERENCES.json")
    known_references = {entry["id"] for entry in references.get("candidates", [])}
    unknown = [ref for ref in task.get("reference_ids", []) if ref not in known_references]
    if unknown:
        problems.append("reference_ids must be candidate ids from tasks/REFERENCES.json, not requirement or issue ids; unknown: "
                        + ", ".join(unknown) + "; known: " + (", ".join(sorted(known_references)) or "none"))
    for reference in task.get("decision_refs", []):
        if not isinstance(reference, str) or not w.inside(root, reference.split("#")[0]).is_file():
            problems.append("decision_refs: missing design decision file " + str(reference))
    if not isinstance(task["gates"], list) or not task["gates"]:
        problems.append("gates must contain at least one required gate with program/args/cwd")
    for gate in task["gates"] if isinstance(task["gates"], list) else []:
        if not isinstance(gate, dict) or not isinstance(gate.get("id"), str) or not gate["id"]:
            problems.append("each gate requires a string id, program, args (list of strings), cwd and required (bool)")
            continue
        if gate.get("required"):
            if not isinstance(gate.get("args"), list) or not all(isinstance(x, str) for x in gate["args"]):
                problems.append("gate " + gate["id"] + ": args must be a list of strings")
            if not gate.get("program") or not gate.get("cwd"):
                problems.append("gate " + gate["id"] + ": required gates need program and cwd (\".\" for the project root)")
            shape = w.gate_argv_problems(gate)
            if shape:
                problems.append("gate " + gate["id"] + ": " + shape)
        elif not gate.get("reason"):
            problems.append("gate " + gate["id"] + ": optional gates need a reason")
    if problems:
        raise ValueError("prepare rejected " + str(len(problems)) + " problem(s):\n- " + "\n- ".join(problems))
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
    baseline = w.check_project(root)["errors"]
    try:
        for name, value in updates.items():
            save(root, name, value)
        must_check(root, [ident], baseline)
    except Exception:
        for name, value in previous.items():
            if value is None:
                w.workflow_inside(root, name).unlink(missing_ok=True)
            else:
                save(root, name, value)
        raise
    write_card(root, task)
    journal(root, "prepare", "任务已冻结：" + str(task["title"]) + "；范围 " + ", ".join(task["scope"]["allowed_paths"]), ident)
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
    journal(root, "research", "参考调研状态 " + status + "，候选 " + str(len(entries)) + " 项：" + research["reason"])
    return {"ok": True, "status": status, "candidates": len(entries), "report": w.workflow_name(root, "tasks/RESEARCH.md")}


def clock_summary(root, task):
    policy = policy_read(root)
    if not w.task_deadline(task):
        return "未开始"
    runs = [read(root, "tasks/runs/" + run_id + ".json") for run_id in task.get("run_ids", [])]
    used = int(w.consumed_seconds(task, runs, w.utc_now()) // 60)
    allowance = int(w.allowance_seconds(task, policy) // 60)
    if w.budget_clock(policy) == "active":
        return f"按活动时间计：已用 {used} 分钟 / 额度 {allowance} 分钟（等待、断网和只读门禁不计）"
    return f"按墙钟计：额度 {allowance} 分钟，写入阶段已用约 {used} 分钟"


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
             "**界面检查**：" + (", ".join(w.ui_check_ids(task)) or "不适用"),
             "**修改范围**：" + ", ".join(task.get("scope", {}).get("allowed_paths", [])), "",
             "## 验收标准", ""] + ["- " + item for item in task.get("acceptance", [])]
    test_review = task.get("test_review")
    if isinstance(test_review, dict):
        baseline = test_review.get("baseline", {})
        labels = {"keep": "保留", "adapt": "适配", "add": "补充", "replace": "替换", "retire": "退役"}
        lines += ["", "## 测试适用性", "",
                  "- 既有行为：" + ("保持" if test_review.get("behavior") == "preserve" else "按已确认需求变化"),
                  "- 原始基线：" + str(baseline.get("status")) + "；" + str(baseline.get("summary")),
                  "- 基线证据：" + str(baseline.get("evidence_ref")),
                  "- 需求决定：" + (", ".join(test_review.get("decision_ids", [])) or "沿用既有行为，无新增业务取舍")]
        for item in test_review.get("actions", []):
            lines.append("- " + labels.get(item.get("action"), "待核对") + "：" + str(item.get("target"))
                         + "；" + str(item.get("reason")) + "；验证：" + (", ".join(item.get("gate_ids", [])) or "对应需求已退役"))
    lines += ["", "## 执行与恢复", "", "- 首次开始：" + str(budget.get("started_at_utc")),
              "- 原截止时间：" + str(budget.get("deadline_at_utc")),
              "- 当前截止时间：" + str(w.task_deadline(task)),
             "- 时钟：" + clock_summary(root, task),
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
    brief = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
    policy = policy_read(root)
    project = read(root, "tasks/PROJECT.json")
    helper = w.workflow_name(root, "scripts/project_workflow.py")
    research_done = w.workflow_inside(root, "tasks/REFERENCES.json").is_file()
    ui_accepted = w.ui_preview_accepted(root, policy, project, tasks)
    save(root, path, w.project_state_bytes(project, tasks, helper, brief=brief, policy=policy,
                                          research_done=research_done, ui_accepted=ui_accepted))
    resume = w.workflow_name(root, "notes/RESUME.md")
    old_resume = w.workflow_inside(root, resume)
    if old_resume.is_file() and not old_resume.read_text(encoding="utf-8-sig").startswith(w.BOARD_MARKER):
        return
    journal_path = w.workflow_inside(root, "notes/JOURNAL.md")
    journal_text = journal_path.read_text(encoding="utf-8-sig") if journal_path.is_file() else ""
    save(root, resume, w.resume_bytes(project, tasks, journal_text, helper, brief=brief, policy=policy,
                                      research_done=research_done, ui_accepted=ui_accepted))


def checkpoint(root, ident, note, next_action):
    if not note or not next_action:
        raise ValueError("Checkpoint needs completed/observed facts and a concrete next action")
    task = task_read(root, ident)
    task.setdefault("checkpoints", []).append({"at_utc": w.iso(w.utc_now()), "note": note, "next_action": next_action})
    journal(root, "checkpoint", note + "；下一步：" + next_action, ident)
    task_save(root, task)
    return {"ok": True, "task_id": ident, "card": w.workflow_name(root, "tasks/cards/") + ident + ".md"}


def remaining_seconds(root, task, strict=True):
    """Seconds left on the task clock: active writer time by default, wall deadline for legacy policies."""
    deadline = w.task_deadline(task)
    if not deadline:
        raise ValueError("Task has not started")
    policy = policy_read(root)
    if w.budget_clock(policy) == "active":
        runs = [read(root, "tasks/runs/" + run_id + ".json") for run_id in task.get("run_ids", [])]
        seconds = w.allowance_seconds(task, policy) - w.consumed_seconds(task, runs, w.utc_now())
        message = "Active-time allowance exhausted; preserve the task and request a scoped budget decision (extend)"
    else:
        seconds = (datetime.fromisoformat(deadline.replace("Z", "+00:00")) - w.utc_now()).total_seconds()
        message = "Original task deadline exhausted; preserve the task and request a scoped budget decision"
    if seconds <= 0 and strict:
        raise ValueError(message)
    return seconds


def clock_exhausted(root, task):
    try:
        return remaining_seconds(root, task, strict=False) <= 0
    except ValueError:
        return False


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
    # Verification and review are read-only gates: they may run after the
    # deadline so an expired clock never hides an already finished candidate.
    remaining_seconds(root, task, strict=kind in {"implementation", "repair", "probe"})
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


def task_baseline(root, task):
    """Baseline for a resumed task: the earlier writer's baseline when nothing else ran since.

    changed_files then means "everything this task changed", so repair rounds
    and resumed runs do not have to re-declare only the latest delta.
    """
    previous = None
    for run_id in reversed(task["run_ids"]):
        run = read(root, "tasks/runs/" + run_id + ".json")
        if run.get("kind") in {"implementation", "repair"} and run.get("scope_manifest"):
            previous = run
            break
    if previous is None:
        return None
    errors = []
    for run in w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors).values():
        if run["task_id"] != task["id"] and run.get("started_at_utc", "") > previous.get("started_at_utc", ""):
            return None
    manifest = w.workflow_inside(root, previous["scope_manifest"])
    return previous["scope_manifest"] if manifest.is_file() else None


def begin(root, ident, context, harness="current_agent"):
    task = task_read(root, ident)
    require_ui_ready(root, task)
    if task["status"] not in {"ready", "blocked"}:
        raise ValueError("Begin requires a ready task or a recorded recoverable blocker")
    if task["status"] == "blocked" and task.get("failure_kind") not in RETRYABLE | {"network", "interrupted", "environment", "budget"}:
        raise ValueError("Recorded blocker '" + str(task.get("failure_kind")) + "' needs a disposition first: run unblock --task "
                         + ident + " --source ... --note ... (or cancel --task), then begin again")
    must_check(root, [ident])
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved" or read(root, "tasks/PROJECT.json")["stage"] in {"intake", "paused", "complete"}:
        raise ValueError("Project authorization/stage does not permit execution")
    if harness != policy["roles"]["coder"]["harness"]:
        raise ValueError("Use the confirmed coder harness; do not silently switch between Agent and CLI")
    permission = {"documentation": "documentation", "baseline": "baseline", "ci": "ci"}.get(task["kind"], "code")
    if policy["authority"].get(permission) is not True:
        raise ValueError("Task execution authority is not enabled")
    if task["input"]["definition_digest"] != w.task_definition(task):
        raise ValueError("Task definition changed; inspect the original task instead of rehashing it")
    if task["status"] == "blocked":
        remaining_seconds(root, task)
    # Capture the scope baseline before any record changes, so the diff at
    # finish only ever contains what happened after this point. A resumed task
    # keeps its earlier baseline so its diff stays cumulative.
    inherited = task_baseline(root, task)
    baseline_files = inventory(root)
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
    scope_manifest = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-scope.json")
    save(root, scope_manifest, {"files": baseline_files})
    run["scope_manifest"] = scope_manifest
    if inherited:
        run["baseline_manifest"] = inherited
    save(root, "tasks/runs/" + run["id"] + ".json", run)
    checkpoint(root, ident, "开始执行，保留原任务身份和截止时间" + ("；沿用本任务先前的范围基线，changed_files 为本任务累计改动" if inherited else ""),
               "完成当前修改后运行 diff --run 核对改动，再调用 finish，然后 verify")
    return {"ok": True, "run_id": run["id"], "task_id": ident, "deadline_at_utc": w.task_deadline(task),
            "allowed_paths": task["scope"]["allowed_paths"], "result_file": w.workflow_name(root, "tasks/evidence/" + run["id"] + "-worker-result.json"),
            "next": "edit only allowed_paths; then diff --run " + run["id"] + " and finish --run " + run["id"] + " --file <worker-result json>"}


def block(root, task, run, reason, kind, exit_code=None):
    # A failed/interrupted writer can still have changed files. Inspect that
    # attempt before another run captures a new baseline and hides the delta.
    if run.get("scope_manifest") and kind != "scope":
        try:
            changed = observed_changes(root, run)
            save(root, "tasks/evidence/" + run["id"] + "-changes.json", {"changed_files": changed})
            groups = classify_changes(task, changed)
            prohibited = changed if run["kind"] == "review" and changed else groups["protected"] + groups["outside"]
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
    recovery_hint = {"scope": "核对 diff --run 列出的越界文件，撤销或用 unblock --note 说明归属后再 begin",
                     "protocol": "按报错列出的漏报/多报文件修正 worker-result，再 unblock 后 begin",
                     "action_required": "处理执行者提出的请求，再 unblock 后 begin",
                     "evidence": "核对证据引用，必要时 recompute；再 unblock"}.get(kind, "先核对已有文件及原始日志，再处理 " + kind)
    checkpoint(root, task["id"], reason, recovery_hint + "；不要新建任务或重置预算")
    repeats = sum(1 for other in read_runs(root).values() if other["id"] != run["id"] and other.get("failure_kind") == kind)
    if kind in {"scope", "protocol", "environment"} and repeats >= 1:
        # The same avoidable failure twice in one project is a lesson, not an accident.
        journal(root, "note/lesson", f"{kind} 失败已出现 {repeats + 1} 次：{reason[:160]}。下次准备/实现前先核对这一点。", task["id"])
    if kind == "network":
        # Freeze the complete partial result after bookkeeping. A later retry
        # must observe exactly this state, including policy and task definition.
        manifest = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-recovery.json")
        save(root, manifest, {"files": inventory(root)})
        run["recovery_manifest"] = manifest
        save(root, "tasks/runs/" + run["id"] + ".json", run)
    return {"ok": False, "task_id": task["id"], "run_id": run["id"], "failure_kind": kind, "error": reason,
            "recover_with": "unblock --task " + task["id"] + " --source ... --note ..." if kind in UNBLOCKABLE - RETRYABLE - {"network"} else "run/begin after the cause is resolved"}


def validate_worker_result(value, task, run):
    required = {"task_id", "run_id", "status", "summary", "changed_files", "validation_requests", "requested_actions", "unresolved_items", "blocked_reason"}
    if not isinstance(value, dict) or set(value) != required:
        raise ValueError("Worker result must match the complete worker-result contract")
    if value["task_id"] != task["id"] or value["run_id"] != run["id"]:
        raise ValueError("Worker result belongs to a different task/run")
    if value["status"] not in {"ready_for_verification", "action_requested", "blocked"} or not isinstance(value["summary"], str):
        raise ValueError("Invalid worker status/summary")
    for key in ("changed_files", "validation_requests", "requested_actions", "unresolved_items"):
        if key == "changed_files" and value[key] == "auto":
            continue
        if not isinstance(value[key], list) or not all(isinstance(item, str) for item in value[key]):
            raise ValueError("Worker " + key + " must be an array of strings" + (" or the string \"auto\"" if key == "changed_files" else ""))
    if value["blocked_reason"] is not None and not isinstance(value["blocked_reason"], str):
        raise ValueError("Invalid blocked_reason")


def read_runs(root):
    errors = []
    return w.records(Path(root).resolve(), "tasks/runs", "RUN-", errors)


def diff_run(root, run_id):
    """Read-only: what changed since the run's baseline, grouped by scope, ready to paste into changed_files."""
    run = read(root, "tasks/runs/" + w.relative_name(run_id) + ".json")
    task = task_read(root, run["task_id"])
    if not run.get("scope_manifest"):
        raise ValueError("This run has no scope baseline")
    changed = observed_changes(root, run)
    groups = classify_changes(task, changed)
    return {"ok": True, "run_id": run_id, "task_id": task["id"], "changed_files": changed,
            "changed_this_run": observed_changes(root, run, since="run"), **groups,
            "allowed_paths": task["scope"]["allowed_paths"],
            "instruction": "changed_files 填 changed_files（本任务累计）或 changed_this_run（本次运行）之一；protected/outside 非空时先撤销或说明归属，否则 finish 会记录 scope 阻塞。"}


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
        changed = observed_changes(root, run)
        save(root, "tasks/evidence/" + run_id + "-changes.json", {"changed_files": changed})
        groups = classify_changes(task, changed)
        prohibited = groups["protected"] + groups["outside"]
        if prohibited:
            return block(root, task, run, "Out-of-scope changes: " + ", ".join(prohibited) + " (allowed: " + ", ".join(task["scope"]["allowed_paths"]) + ")", "scope")
        declared = set(changed) if result["changed_files"] == "auto" else set(result["changed_files"])
        if result["changed_files"] == "auto":
            result = {**result, "changed_files": changed, "changed_files_source": "auto: filled from diff by the tool"}
            save(root, result_path, result)
        this_run = observed_changes(root, run, since="run")
        if declared != set(changed) and declared != set(this_run):
            undeclared, extra = sorted(set(changed) - declared), sorted(declared - set(changed))
            detail = []
            if undeclared:
                detail.append("observed but not declared (including deletions): " + ", ".join(undeclared))
            if extra:
                detail.append("declared but unchanged: " + ", ".join(extra))
            return block(root, task, run, "Worker changed_files does not match the observed diff; " + "; ".join(detail)
                         + "; declare either the task's cumulative changes " + json.dumps(changed, ensure_ascii=False)
                         + " or this run's changes " + json.dumps(this_run, ensure_ascii=False), "protocol")
        if result["status"] != "ready_for_verification" or result["requested_actions"] or result["unresolved_items"] or result["blocked_reason"]:
            return block(root, task, run, result["blocked_reason"] or "Worker requests manager action; inspect the result", "action_required")
        remaining_seconds(root, task)
        snapshot = selected_snapshot(root, task)
        manifest = w.workflow_name(root, "tasks/evidence/" + run_id + "-candidate.json")
        save(root, manifest, snapshot)
        run.update(outcome="completed", finished_at_utc=w.iso(w.utc_now()), exit_code=0, candidate_digest=snapshot["digest"])
        task["evidence"].update(candidate_manifest=manifest, candidate_digest=snapshot["digest"], verification_run=None, review_run=None)
        task.update(status="verifying", blockers=[])
        save(root, "tasks/runs/" + run_id + ".json", run)
        task_save(root, task)
        checkpoint(root, task["id"], "编码结果已记录，差异范围已核对：" + (", ".join(changed) or "无文件变化"), "运行 verify；代码完成尚未等于验收通过")
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
        raise ValueError("Recorded blocker '" + str(task.get("failure_kind")) + "' needs a disposition first: run unblock --task " + ident + " --source ... --note ...")
    if not task["evidence"].get("candidate_manifest"):
        raise ValueError("No candidate recorded yet; finish an implementation run first")
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
            left = remaining_seconds(root, task, strict=False)
            timeout = float(gate.get("timeout_seconds", 300))
            if left > 0:
                timeout = min(left, timeout)
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
        result = {"ok": True, "task_id": ident, "run_id": run["id"], "candidate_digest": snapshot["digest"], "next": "review"}
        hint = snapshot_hint(root, task, snapshot)
        if hint:
            result["snapshot_hint"] = hint
        return result
    except (ValueError, OSError, KeyError, TypeError) as error:
        return block(root, task, run, str(error), "environment")


def snapshot_hint(root, task, snapshot):
    """Suggest a narrower snapshot when a task froze far more than it touched.

    Wide roots make every later edit in those roots invalidate this candidate,
    which is the main cause of chained re-verification in real projects.
    """
    changed = set()
    for run_id in task["run_ids"]:
        path = w.workflow_inside(root, "tasks/evidence/" + run_id + "-changes.json")
        if path.is_file():
            changed.update(w.read_json(path).get("changed_files", []))
    frozen = len(snapshot["files"])
    if frozen >= 15 and changed and len(changed) * 5 <= frozen:
        return {"frozen_files": frozen, "changed_files": len(changed), "roots": snapshot["roots"],
                "suggestion": "后续任务把 snapshot_paths 收窄到实际会改的文件/目录（本任务只改了 " + str(len(changed)) + " 个文件，却冻结了 "
                              + str(frozen) + " 个）；范围太宽会让后续任务的改动反复使本候选失效。"}
    return None


def review(root, ident, report, context, mode="self_review", existing_run=None):
    task = task_read(root, ident)
    if task["status"] != "review":
        raise ValueError("Complete verification before review")
    policy = policy_read(root)
    if policy["approval"]["status"] != "approved":
        raise ValueError("Project authorization does not permit review")
    if policy["review"].get("evidence_version") != 1:
        raise ValueError("Review evidence requirements cannot be disabled; inspect an explicit record migration")
    if existing_run is None and policy["roles"]["reviewer"]["harness"] != "current_agent":
        raise ValueError("Use the confirmed reviewer harness; do not silently replace CLI review with current Agent review")
    if mode not in {"self_review", "independent"}:
        raise ValueError("Unknown review mode")
    if w.required_review_mode(policy, task) == "independent_required" and mode != "independent":
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
    quality_digest = None
    if report["verdict"] == "PASS" and not report["findings"] and policy["review"].get("evidence_version") == 1:
        quality_digest = w.review_quality_digest(root, task, report)
    baseline_errors = w.check_project(root)["errors"]
    run = existing_run or start_run(root, task, "review", context, "current_agent")
    path = w.workflow_name(root, "tasks/evidence/" + run["id"] + "-review.json")
    save(root, path, report)
    run.update(candidate_digest=current)
    run["raw_output_refs"].append(path)
    run["review"] = {"mode": mode, "verdict": report["verdict"], "report": path, "candidate_digest": current}
    if ui_digest:
        run["review"]["ui_evidence_digest"] = ui_digest
    if quality_digest:
        run["review"]["quality_digest"] = quality_digest
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
    own_errors = w.relevant_errors(check["errors"], [ident], baseline_errors)
    if own_errors:
        task.update(status="blocked", blockers=own_errors, failure_kind="evidence")
        task_save(root, task)
        return {**check, "ok": False, "errors": own_errors, "task_id": ident, "failure_kind": "evidence"}
    checkpoint(root, ident, "当前候选的测试与审查通过（" + mode + "）", "继续已授权任务；所属功能完成后请用户验收")
    return {"ok": True, "task_id": ident, "status": "verified", "next": "next"}


def unblock(root, ident, source, note_text, resume_to=None):
    """Owner-backed disposition for a blocked task; the task keeps its identity, clock and history."""
    if not source or not source.strip() or not note_text or not note_text.strip():
        raise ValueError("unblock needs --source (who decided) and --note (what was inspected and why it is safe to continue)")
    task = task_read(root, ident)
    if task["status"] != "blocked":
        raise ValueError("Only a blocked task can be unblocked; current status is " + task["status"])
    if any(read(root, "tasks/runs/" + run_id + ".json")["outcome"] == "running" for run_id in task["run_ids"]):
        raise ValueError("Recover the active run before unblocking")
    kind = task.get("failure_kind")
    has_candidate = bool(task["evidence"].get("candidate_manifest"))
    default = "verifying" if kind in {"evidence"} or (kind == "interrupted" and task.get("resume_phase") in {"verification", "review"} and has_candidate) else "ready"
    target = resume_to or default
    if target not in {"ready", "verifying"}:
        raise ValueError("--to must be ready (re-run implementation) or verifying (re-run verification on the existing candidate)")
    if target == "verifying" and not has_candidate:
        raise ValueError("No candidate exists; unblock to ready instead")
    if target == "ready" and remaining_seconds(root, task, strict=False) <= 0:
        raise ValueError("Task clock is exhausted; run extend --task " + ident + " first, then unblock")
    task.setdefault("dispositions", []).append({"at_utc": w.iso(w.utc_now()), "failure_kind": kind, "blockers": task.get("blockers", []),
                                                "source": source, "note": note_text, "resumed_to": target})
    task.update(status=target, blockers=[])
    task.pop("failure_kind", None)
    if target == "ready":
        task["evidence"].update(verification_run=None, review_run=None)
    task_save(root, task)
    checkpoint(root, ident, "阻塞已处置（" + str(kind) + "）：" + note_text, "begin 重新实现" if target == "ready" else "verify 当前候选")
    journal(root, "unblock", str(kind) + " → " + target + "；依据：" + source, ident)
    return {"ok": True, "task_id": ident, "status": target, "next": "begin" if target == "ready" else "verify"}


def cancel(root, ident, reason, source):
    if not reason or not reason.strip() or not source or not source.strip():
        raise ValueError("cancel needs --reason and --source")
    task = task_read(root, ident)
    if task["status"] in {"done", "cancelled"}:
        raise ValueError("Task is already " + task["status"])
    if any(read(root, "tasks/runs/" + run_id + ".json")["outcome"] == "running" for run_id in task["run_ids"]):
        raise ValueError("Recover the active run before cancelling")
    errors = []
    tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
    dependents = [other["id"] for other in tasks.values() if ident in other.get("dependencies", []) and other["status"] not in {"done", "cancelled"}]
    if dependents:
        raise ValueError("Cancel or re-plan dependent tasks first: " + ", ".join(dependents))
    for dependency in task.get("continuation_of", {}):
        prior = task_read(root, dependency)
        if prior["evidence"].get("continued_by") == ident:
            prior["evidence"]["continued_by"] = None
            task_save(root, prior)
    task.setdefault("dispositions", []).append({"at_utc": w.iso(w.utc_now()), "cancelled": True, "reason": reason, "source": source})
    task.update(status="cancelled", blockers=[])
    task.pop("failure_kind", None)
    project = read(root, "tasks/PROJECT.json")
    if project.get("current_task") == ident:
        project["current_task"] = None
        save(root, "tasks/PROJECT.json", project)
    task_save(root, task)
    checkpoint(root, ident, "任务已取消：" + reason, "如需同一目标，准备新的任务并引用本任务作为历史")
    journal(root, "cancel", reason + "；依据：" + source, ident)
    return {"ok": True, "task_id": ident, "status": "cancelled"}


def recompute(root, ident, source):
    """Recompute derived review digests with the current tool after an audited tool change."""
    if not source or not source.strip():
        raise ValueError("recompute needs --source describing the tool change that made the digests stale")
    task = task_read(root, ident)
    review_run_id = task["evidence"].get("review_run")
    if not review_run_id:
        raise ValueError("Task has no recorded review run")
    run = read(root, "tasks/runs/" + review_run_id + ".json")
    report = read(root, run["review"]["report"])
    changed = {}
    if run["review"].get("verdict") == "PASS":
        new_quality = w.review_quality_digest(root, task, report)
        if run["review"].get("quality_digest") != new_quality:
            changed["quality_digest"] = [run["review"].get("quality_digest"), new_quality]
            run["review"]["quality_digest"] = new_quality
        if task.get("ui_change"):
            new_ui = w.ui_review_digest(root, task, report)
            if run["review"].get("ui_evidence_digest") != new_ui:
                changed["ui_evidence_digest"] = [run["review"].get("ui_evidence_digest"), new_ui]
                run["review"]["ui_evidence_digest"] = new_ui
    if changed:
        run.setdefault("recomputed", []).append({"at_utc": w.iso(w.utc_now()), "source": source, "changed": changed})
        save(root, "tasks/runs/" + review_run_id + ".json", run)
        journal(root, "recompute", "重算派生摘要 " + ", ".join(changed) + "；依据：" + source, ident)
    if task["status"] == "blocked" and task.get("failure_kind") == "evidence":
        remaining = w.relevant_errors(w.check_project(root)["errors"], [ident])
        if not remaining:
            task.update(status="verified", blockers=[])
            task.pop("failure_kind", None)
            task_save(root, task)
    return {"ok": True, "task_id": ident, "changed": changed, "status": task_read(root, ident)["status"]}


def record_ui_feedback(root, ident, note, source):
    """A rejected preview continues in its original task, with the same budget."""
    must_check(root, [ident])
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
    # Only the brief facts this task needs; the full interview stays on disk.
    refs = set(task.get("requirement_refs", []))
    trimmed_brief = {key: brief.get(key) for key in ("goal", "audience", "acceptance", "non_goals", "compatibility", "quality") if key in brief}
    trimmed_brief["requirements"] = [item for item in brief.get("requirements", []) if isinstance(item, dict) and item.get("id") in refs]
    if brief.get("ui", {}).get("mode") not in {None, "none", "pending"}:
        trimmed_brief["ui"] = brief.get("ui")
    trimmed_task = {key: value for key, value in task.items() if key not in {"checkpoints", "dispositions", "feedback"}}
    context = {"task_id": task["id"], "run_id": run["id"], "task": trimmed_task, "brief": trimmed_brief,
               "reference_candidates": [item for item in research.get("candidates", []) if item["id"] in task.get("reference_ids", [])],
               "read_first": ["AGENTS.md", "WORKFLOW-KIT.md" if w.inside(root, w.ISOLATED_BINDING).is_file() else "WORKFLOW.md"], "candidate_digest": task["evidence"].get("candidate_digest"),
               "deadline_at_utc": w.task_deadline(task)}
    if review_only:
        context["role"] = "reviewer"
        context["verification"] = read(root, "tasks/runs/" + task["evidence"]["verification_run"] + ".json")
        context["verification_ref"] = w.workflow_name(root, "tasks/runs/" + task["evidence"]["verification_run"] + ".json")
        context["required_review_areas"] = list(w.REVIEW_AREAS)
        context["required_review_mode"] = w.required_review_mode(policy_read(root), task)
        context["candidate_files"] = read(root, task["evidence"]["candidate_manifest"])["files"]
        for run_id in reversed(task.get("run_ids", [])):
            candidate = w.workflow_inside(root, "tasks/evidence/" + run_id + "-changes.json")
            if candidate.is_file():
                context["changed_files"] = w.read_json(candidate).get("changed_files", [])
                break
    if task.get("ui_change"):
        context["read_first"] += [task["ui_contract_ref"], w.workflow_name(root, "docs/workflow/FRONTEND.md")]
        context["ui_evidence_directory"] = w.workflow_name(root, "tasks/evidence/") + task["id"] + "-ui/" + str(task["evidence"].get("verification_run") or "pending") + "/"
    return json.dumps({"instructions": instruction, "task_packet": context}, ensure_ascii=False, indent=2)


def review_packet(root, ident):
    """Read-only neutral handoff; the host supplies a genuinely fresh reviewer."""
    must_check(root, [ident])
    task = task_read(root, ident)
    if task["status"] != "review":
        raise ValueError("Complete verification before preparing review inputs")
    return {"ok": True, **json.loads(packet(root, task, {"id": None}, review_only=True)),
            "instruction": "把此输入交给未参与实现的新上下文；同模型即可，不要求 CLI。不要复制作者会话或通过预期。不能创建独立上下文时如实报告，按已确认审查策略处理。"
                           " 审查证据只能引用候选文件或 tasks/runs、tasks/evidence 下的附件，不要引用任务记录、卡片或决定文件。"}


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
        must_check(root, [ident])
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
        timeout = min(remaining_seconds(root, task), w.recovery_policy(policy)["max_cli_call_seconds"])
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
        if delay >= remaining_seconds(root, task):
            raise ValueError("Not enough original task time remains for retry backoff")
    except ValueError as error:
        return {**denied, "next": "budget_decision", "reason": str(error)}
    return {"allowed": True, "phase": phase, "attempt": len(streak), "max_retries": len(delays),
            "failed_run": failed["id"], "retry_at_utc": w.iso(due), "wait_seconds": delay}


def retry_network(root, task):
    must_check(root, [task["id"]])
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
            must_check(root, [ident])
            return {"ok": True, "task_id": ident, "status": task["status"]}
        if task["status"] == "blocked" and task.get("failure_kind") == "network":
            result = retry_network(root, task)
            if result.get("retry_stopped"):
                return result
        elif task["status"] in {"ready", "blocked"}:
            if task["status"] == "blocked" and task.get("failure_kind") not in RETRYABLE:
                return {"ok": False, "task_id": ident, "failure_kind": task.get("failure_kind"),
                        "error": "Inspect and resolve the interruption/environment before continuing",
                        "recover_with": "unblock --task " + ident + " --source ... --note ... (or cancel)"}
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


READ_NEXT = {
    "ask": ["docs/workflow/INTAKE.md"], "assess_existing": ["docs/workflow/REFACTOR.md"], "onboard": ["docs/workflow/INTAKE.md"],
    "research": ["docs/workflow/RESEARCH.md"], "prepare": ["docs/workflow/TOOLING.md", "docs/workflow/GATES.md"],
    "prepare_ui_preview": ["docs/workflow/FRONTEND.md"], "accept_ui_preview": ["docs/workflow/FRONTEND.md"],
    "begin": ["docs/workflow/prompts/WORKER.md"], "run": ["docs/workflow/EXECUTION.md"], "verify": ["docs/workflow/GATES.md"],
    "review": ["docs/workflow/prompts/REVIEW.md", "docs/workflow/adapters/SINGLE-AGENT.md"], "repair": ["docs/workflow/EXECUTION.md"],
    "replan": ["docs/workflow/RECOVERY.md"], "unblock": ["docs/workflow/RECOVERY.md"], "inspect_blocker": ["docs/workflow/RECOVERY.md"],
    "inspect_active_run": ["docs/workflow/HANDOFF.md"], "recover_lock": ["docs/workflow/HANDOFF.md"], "budget_decision": ["docs/workflow/RECOVERY.md"],
    "accept_or_prepare_next_authorized_task": ["docs/workflow/LIFECYCLE.md"], "prepare_or_finish_project": ["docs/workflow/LIFECYCLE.md"],
}


def next_action(root):
    integration = boot.integration_status(root)
    if not integration["connected"] and integration["status"] != "not_connected":
        return {"ok": False, "next": "bootstrap" if integration["status"] == "foreign_workflow" else "repair_integration",
                "integration": integration, "errors": integration["errors"],
                "questions": intake_flow.initial_questions("refactor") if integration["status"] == "foreign_workflow" else [],
                "instruction": "尚未接入当前 workflow-kit；不要沿旧工作流继续，也不要清空旧任务。先完成可核对的接入。"}
    result = _next_action(root)
    result["integration"] = integration
    if integration["connected"] and result.get("next") in READ_NEXT:
        result["read_next"] = [w.workflow_name(root, name) for name in READ_NEXT[result["next"]]]
        result["context_note"] = "只读 read_next 列出的文档；其余文档按需再读。日常轮次用 presentation.compact，不要把完整 JSON 或整份 BRIEF 贴进对话。"
    if integration["connected"] and w.workflow_inside(root, "tasks/PROJECT.json").is_file() and "record_errors" not in result:
        try:
            grouped = {}
            for message in w.check_project(root)["errors"]:
                grouped.setdefault(w.error_owner(message) or "global", []).append(message)
            if grouped:
                result["record_errors"] = grouped
                result["record_errors_instruction"] = "这些记录/证据问题按任务分区，不阻止其他任务；对应任务用 recompute、unblock 或核对记录处理，不清空历史。"
        except (ValueError, OSError, KeyError, TypeError):
            pass
    if integration["connected"]:
        try:
            if not result.get("ok"):
                result["presentation"] = {"markdown": "**进度暂不能确认**：工作流记录或证据未通过校验。\n\n" + "\n".join("- " + str(item) for item in result.get("errors", [])),
                                          "display_instruction": "说明具体记录/证据缺口，不能把历史完成状态显示为当前已完成；保留原任务排障。"}
                return result
            errors = []
            tasks = w.records(Path(root).resolve(), "tasks/items", "TASK-", errors)
            project, policy = read(root, "tasks/PROJECT.json"), policy_read(root)
            brief = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
            prefix = (Path(root).resolve() / w.workflow_name(root, "tasks/cards")).as_posix() + "/"
            result["presentation"] = progress_view.build(project, policy, brief, tasks, result, card_prefix=prefix,
                research_done=w.workflow_inside(root, "tasks/REFERENCES.json").is_file(),
                ui_accepted=w.ui_preview_accepted(root, policy, project, tasks))
        except (ValueError, OSError, KeyError, TypeError) as error:
            result["presentation"] = {"markdown": "进度记录需核对：" + str(error), "display_instruction": "说明当前记录缺口，不猜测完成度。"}
    else:
        result["presentation"] = {"markdown": "当前项目尚未核验接入；先确认目标和已有项目意向，再按启动入口继续。",
                                  "display_instruction": "展示当前状态并提出必要问题，不声称已经开始开发。"}
    return result


def resume(root):
    """One command for a fresh Agent: what this is, where it stands, what to do, what to paste."""
    result = next_action(root)
    integration = result.get("integration", {})
    helper = integration.get("helper") or "scripts/project_workflow.py"
    lines = ["# 接手", ""]
    if not integration.get("connected"):
        lines += ["接入状态：**" + str(integration.get("status")) + "**", ""]
        lines += ["- " + str(item) for item in integration.get("errors", [])] or ["- 尚未接入：按 START 运行 bootstrap，或用 rebind 修复。"]
        return {"ok": False, "markdown": "\n".join(lines) + "\n", "next": result.get("next"), "integration": integration}
    presentation = result.get("presentation", {})
    lines += [presentation.get("compact") or presentation.get("markdown", ""), ""]
    lines += ["**身份**：当前会话是总控；只有明确的 task_id/run_id 执行包才是 Worker。", ""]
    if result.get("record_errors"):
        lines += ["**记录问题（按任务分区，不阻止其他任务）**："] + ["- " + key + "：" + "；".join(value[:2]) for key, value in result["record_errors"].items()] + [""]
    notes_path = w.workflow_inside(root, "notes/JOURNAL.md")
    if notes_path.is_file():
        entries = [line for line in notes_path.read_text(encoding="utf-8-sig").splitlines() if " · note/" in line][-6:]
        if entries:
            lines += ["**Agent 留下的判断与待办**："] + entries + [""]
    commands = {"ask": "intake --file <本轮答案.json> --write", "onboard": "onboard --file <确认后的 brief.json> --source \"<用户原话>\"",
                "research": "research --file <调研记录.json>", "prepare": "prepare --file <任务规格.json>",
                "begin": "begin --task " + str(result.get("task_id")) + " --context <本会话标识>",
                "verify": "verify --task " + str(result.get("task_id")), "review": "review-packet --task " + str(result.get("task_id")),
                "run": "run --task " + str(result.get("task_id")), "repair": "begin --task " + str(result.get("task_id")) + " --context <本会话标识>",
                "unblock": "unblock --task " + str(result.get("task_id")) + " --source \"<决定来源>\" --note \"<核对说明>\"",
                "budget_decision": "extend --task " + str(result.get("task_id")) + " --minutes <N> --source \"<用户决定>\"",
                "inspect_active_run": "recover --run " + str(result.get("run_id")) + " --source \"<核对进程后的说明>\"（仅当进程已退出）",
                "recover_lock": "recover --source \"<核对控制器已退出>\"",
                "accept_or_prepare_next_authorized_task": "accept --tasks <TASK-ids> --source \"<用户验收原话>\" --merge-ref <ref|not_applicable:...>",
                "prepare_or_finish_project": "prepare --file <下一任务规格.json> 或 accept --tasks ... --project-complete",
                "accept_ui_preview": "accept --tasks " + str(result.get("task_id")) + " ... 或 feedback --task " + str(result.get("task_id")) + " --note ... --source ...",
                "replan": "note --kind decision --text \"<新方法>\" 然后 begin --task " + str(result.get("task_id"))}
    lines += ["**下一步**：" + str(presentation.get("next_action") or result.get("next")), ""]
    if result.get("next") in commands:
        lines += ["```text", "python " + helper + " " + commands[result["next"]] + " --root .", "```", ""]
    if result.get("instruction"):
        lines += [str(result["instruction"]), ""]
    if result.get("read_next"):
        lines += ["**先读**：" + "、".join(result["read_next"]) + "；其余文档按需再读。", ""]
    lines += ["完整状态：`progress`；任务卡：" + w.workflow_name(root, "tasks/cards/") + "；日志：" + w.workflow_name(root, "notes/JOURNAL.md") + "。",
              "每完成一个可恢复步骤就 checkpoint；影响后续判断的事实用 note 记下；大项目建议一个会话只做一张任务卡。"]
    return {"ok": True, "markdown": "\n".join(lines) + "\n", "next": result.get("next"), "task_id": result.get("task_id"),
            "needs_user": presentation.get("needs_user", []), "native_tasks": presentation.get("native_tasks", []),
            "native_plan": presentation.get("native_plan", []), "read_next": result.get("read_next", []), "integration": integration}


def _next_action(root):
    root = Path(root).resolve()
    if not w.workflow_inside(root, "tasks/PROJECT.json").is_file():
        existing = root.is_dir() and any(path.name not in {".git", ".gitignore", ".gitkeep"} for path in root.iterdir())
        return {"ok": True, "stage": "intake", "next": "ask", "questions": intake_flow.initial_questions("refactor" if existing else "new"),
            "instruction": "先只读辨认目录；已有项目先问重构意向。按已选择工作流完成 bootstrap 接入，再保存分轮答案；不自动授权代码。"}
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
        if deadline and task["status"] in {"ready", "running"}:
            if clock_exhausted(root, task):
                return {"ok": True, "next": "budget_decision", "task_id": task["id"], "deadline_at_utc": deadline,
                        "instruction": "原任务期限已耗尽。保留现场；取得延长预算的明确决定后用 extend，不重建任务。"}
    check = w.check_project(root)
    global_errors = [message for message in check["errors"] if w.error_owner(message) is None]
    if global_errors:
        return {**check, "errors": global_errors, "next": "repair_records_or_refresh_stale_evidence", "instruction": "保持已有身份、记录和代码；不能重初始化清空问题。"}
    record_errors = {}
    for message in check["errors"]:
        record_errors.setdefault(w.error_owner(message), []).append(message)
    if project["stage"] in {"paused", "complete"}:
        return {"ok": True, "stage": project["stage"], "next": project["stage"], "record_errors": record_errors}
    if policy["approval"]["status"] != "approved":
        return intake(root)
    if project["kind"] == "refactor" and not policy["authority"]["code"] and not tasks:
        brief = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
        if brief.get("refactor", {}).get("decision") in {"keep", "assess_only"}:
            return {"ok": True, "next": "assessment_complete", "instruction": "展示已确认的评估结论与适用范围；无需制造编码任务。用户以后决定实施时记录新决定，保留现有记录。"}
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
        if task["status"] == "blocked" and task.get("failure_kind") in {"scope", "protocol", "action_required", "evidence"}:
            action = "unblock"
        if task["status"] == "blocked" and w.task_deadline(task):
            if clock_exhausted(root, task) and task.get("failure_kind") != "evidence":
                action = "budget_decision"
        if record_errors.get(task["id"]):
            action = "repair_records_or_refresh_stale_evidence"
        result = {"ok": True, "task_id": task["id"], "status": task["status"], "next": action,
                "card": w.workflow_name(root, "tasks/cards/") + task["id"] + ".md", "blockers": task["blockers"],
                "failure_kind": task.get("failure_kind"), "record_errors": record_errors.get(task["id"], []),
                "other_record_errors": {key: value for key, value in record_errors.items() if key != task["id"]},
                "deadline_at_utc": w.task_deadline(task), "checkpoints": task.get("checkpoints", [])[-3:],
                "resume_with": "verify" if task.get("resume_phase") in {"verification", "review"} else "begin"}
        if action == "unblock":
            result["instruction"] = ("阻塞类型 " + str(task.get("failure_kind")) + " 需要处置记录：先读任务卡最近检查点和 diff/changes 证据，"
                                     "撤销越界改动或确认归属，再运行 unblock --task " + task["id"] + " --source <决定来源> --note <核对说明>；不要新建任务。")
        if action == "repair_records_or_refresh_stale_evidence":
            result["instruction"] = "该任务的记录或证据未通过校验；核对下列错误后修正记录或用 recompute/unblock 处理，不要清空历史。"
        if task["status"] == "review":
            result["review_mode"] = w.required_review_mode(policy, task)
            result["instruction"] = ("用 review-packet 提供中立输入；交给未参与实现的新上下文审查，同模型和无 CLI 均可。没有能力时明确阻塞，不能改名字自审。"
                if result["review_mode"] == "independent_required" else "使用真实验证与五项有证据的审查；自审必须明确标记，不把 PASS 摘要代替检查。")
        return result
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
    instruction = "Agent 将已确认范围拆成可验证小任务；不要让用户手填任务 JSON。"
    if project["kind"] == "refactor":
        instruction += " 先按 GATES 对照当前需求审查旧测试/门禁，保存原基线并填写 test_review，再准备实施任务。"
    return {"ok": True, "next": "prepare", "instruction": instruction}


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
    baseline = w.check_project(root)["errors"]
    try:
        save(root, "tasks/DECISIONS.json", decisions)
        task_save(root, task)
        must_check(root, [ident], baseline)
    except Exception:
        save(root, "tasks/DECISIONS.json", previous_decisions)
        task_save(root, previous_task)
        raise
    checkpoint(root, ident, "依据新决定追加预算；原始时钟与失败记录保留", "先核对已有成果，再按原任务范围继续")
    journal(root, "extend", "追加 " + str(minutes) + " 分钟、" + str(repair_rounds) + " 轮修复；依据：" + source, ident)
    return {"ok": True, "task_id": ident, "decision_id": decision_id, "deadline_at_utc": w.task_deadline(task)}


def new_batch(root, source=None):
    project = read(root, "tasks/PROJECT.json")
    policy = policy_read(root)
    if not project.get("current_batch") or project["stage"] in {"intake", "paused", "complete"}:
        raise ValueError("No active delivery batch to continue")
    previous = read(root, "tasks/batches/" + project["current_batch"] + ".json")
    must_check(root, previous["task_ids"])
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
    journal(root, "batch", "关闭 " + previous["id"] + "，开启 " + ident + ("；依据：" + source if source else "；策略允许自动续批"))
    refresh_project_state(root)
    return {"ok": True, "batch_id": ident, "previous_batch": previous["id"], "next": "prepare"}


def accept(root, identifiers, source, merge_ref, project_complete=False):
    must_check(root, identifiers)
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
        brief = read(root, "tasks/BRIEF.json") if w.workflow_inside(root, "tasks/BRIEF.json").is_file() else {}
        covered = {ref for task in all_tasks.values() if task["status"] == "done" or task["id"] in identifiers for ref in task.get("requirement_refs", [])}
        missing = [item["id"] for item in brief.get("requirements", []) if item.get("in_scope") and item["id"] not in covered]
        if missing:
            raise ValueError("Project still has confirmed requirements without accepted tasks: " + ", ".join(missing))
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
    # The strict commit gate cannot pass for evidence written a moment ago; it
    # applies from the next check on, so the owner commits before continuing.
    check = w.check_project(root)
    blocking = [message for message in w.relevant_errors(check["errors"], identifiers) if "acceptance evidence" not in message]
    if blocking:
        raise ValueError("; ".join(blocking))
    journal(root, "accept", ("项目整体验收" if project_complete else "验收 " + ", ".join(identifiers)) + "；依据：" + source)
    refresh_views(root)
    result = {"ok": True, "accepted": identifiers, "decision_id": decision["id"], "next": "next"}
    if policy_read(root).get("acceptance", {}).get("require_committed_evidence") is True:
        result["commit_required"] = [evidence] + [w.workflow_name(root, "tasks/items/" + task["id"] + ".json") for task in tasks]
        result["instruction"] = "策略要求验收记录入库：提交上述文件后再继续，否则后续 check/next 会持续报告该任务的记录问题。"
    return result


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
        if command == "intake":
            item.add_argument("--file", help="Optional partial answers prepared by the Agent")
            item.add_argument("--write", action="store_true", help="Save draft answers only; never approve work")
        if command in {"progress", "start", "next", "resume"}:
            item.add_argument("--format", choices=("json", "markdown", "compact"),
                              default="markdown" if command in {"progress", "resume"} else "json",
                              help="markdown: full board; compact: ten-line summary for routine turns; json: everything")
        if command == "review-packet":
            item.add_argument("--task", required=True)
        if command in {"onboard", "recover", "accept", "extend", "feedback", "unblock", "cancel", "recompute", "rebind"}:
            item.add_argument("--source", required=True, help="Reference to the actual user approval/recovery decision")
        if command == "batch":
            item.add_argument("--source", help="Actual owner decision, unless existing policy permits automatic rollover")
        if command in {"begin", "verify", "review", "feedback", "dispatch", "review-cli", "run", "checkpoint", "extend", "unblock", "cancel", "recompute"}:
            item.add_argument("--task", required=True)
        if command == "note":
            item.add_argument("--task", help="Optional task the note belongs to")
            item.add_argument("--kind", choices=NOTE_KINDS, required=True)
            item.add_argument("--text", required=True)
        if command == "unblock":
            item.add_argument("--note", required=True, help="What was inspected and why continuing is safe")
            item.add_argument("--to", choices=("ready", "verifying"), help="Phase to resume; default derives from the blocker")
        if command == "cancel":
            item.add_argument("--reason", required=True)
        if command == "diff":
            item.add_argument("--run", required=True)
        if command == "rebind":
            item.add_argument("--upgrade-tools", action="store_true", help="Also copy the calling package's engine scripts into the project")
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
        if command in {"start", "next", "progress"}:
            return next_action(root)
        if command == "resume":
            return resume(root)
        if command == "review-packet":
            return review_packet(root, args.task)
        if command == "cards":
            return refresh_views(root)
        if command == "adopt":
            return adopt(root, args.kind, args.name, args.write, args.full_docs)
        if command == "onboard":
            return onboard(root, w.read_json(args.file), args.source)
        if command == "intake":
            return intake(root, w.read_json(args.file) if args.file else None, args.write)
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
        if command == "unblock":
            return unblock(root, args.task, args.source, args.note, args.to)
        if command == "cancel":
            return cancel(root, args.task, args.reason, args.source)
        if command == "diff":
            return diff_run(root, args.run)
        if command == "note":
            return note(root, args.kind, args.text, args.task)
        if command == "recompute":
            return recompute(root, args.task, args.source)
        if command == "rebind":
            return boot.rebind(root, args.source, args.upgrade_tools)
        return accept(root, args.tasks, args.source, args.merge_ref, args.project_complete)

    try:
        if args.command in {"bootstrap", "doctor", "start", "next", "progress", "review-packet", "adopt", "recover", "diff", "rebind", "resume"} or (args.command == "intake" and not args.write):
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
        fmt = getattr(args, "format", "json")
        if args.command == "resume" and fmt != "json":
            print(result.get("markdown") or "当前接入或状态需要处理，请先查看 start 输出。")
        elif args.command in {"progress", "start", "next"} and fmt != "json":
            presentation = result.get("presentation", {})
            text = presentation.get("compact" if fmt == "compact" else "markdown")
            print(text or "当前接入或状态需要处理，请先查看 start 输出。")
        else:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0 if result.get("ok") else 1
    except (ValueError, OSError, KeyError, TypeError, AttributeError, subprocess.SubprocessError) as error:
        print(json.dumps({"ok": False, "errors": [str(error)]}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
