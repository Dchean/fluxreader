#!/usr/bin/env python3
"""Project records and validation. Execution commands delegate to workflow_runtime."""
# Note: 非破坏初始化与可核对证据 — 包内 ../.agents/notes/implemented/process/
# 2026-09-13-portable-project-workflow.md；生成项目见 ../docs/workflow/design-note.md。

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path, PurePosixPath

VERSION = 1
# Release of the startup package; stored in each project binding so upgrades are visible.
KIT_RELEASE = "2026-09-18.3"
BOARD_MARKER = "<!-- project-workflow: generated view; edit task JSON instead -->"
TASK_STATES = {"draft", "ready", "running", "verifying", "review", "verified", "blocked", "done", "cancelled"}
ACTIVE = {"ready", "running", "verifying", "review"}
PREPARED = ACTIVE | {"verified", "done"}
RUN_KINDS = {"implementation", "repair", "probe", "verification", "review"}
GATE_STATES = {"PASS", "FAIL", "NOT_RUN", "SKIPPED", "BLOCKED", "NOT_APPLICABLE"}
REVIEW_AREAS = ("requirements", "regression", "failure_paths", "maintainability", "performance")
DENIED_PARTS = {".git", ".claude", ".codex", "node_modules", ".venv", "venv", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache"}
SECRET_NAMES = {"auth.json", "credentials.json", "run-settings.json"}
SECRET_SUFFIXES = (".pem", ".key", ".p12", ".pfx")
DATA_SUFFIXES = (".db", ".sqlite", ".sqlite3")
RECOVERY_DEFAULTS = {"network_backoff_seconds": [5, 15], "max_cli_call_seconds": 600}
# Records the tool itself rewrites while a task is open. They are never task
# evidence and never count as a worker's change.
MUTABLE_RECORDS = ("tasks/items/", "tasks/cards/", "tasks/runtime/", "notes/")
MUTABLE_FILES = {"tasks/DECISIONS.json", "tasks/PROJECT.json", "tasks/PROJECT_STATE.md", "tasks/BACKLOG.md",
                 "tasks/IN_PROGRESS.md", "tasks/DONE.md", "tasks/REFERENCES.json", "tasks/RESEARCH.md"}
ISOLATED_ROOT = ".workflow-kit"
ISOLATED_BINDING = ".workflow-kit/binding.json"
CLASSIC_BINDING = "tasks/WORKFLOW_KIT.json"


def utc_now():
    return datetime.now(timezone.utc)


def iso(value):
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def read_json(path):
    value = json.loads(Path(path).read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected JSON object: {path}")
    return value


def encoded(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def digest(value):
    data = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(data).hexdigest()


def relative_name(value, pattern=False):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        raise ValueError(f"Expected portable relative path: {value!r}")
    parts = PurePosixPath(value)
    if parts.is_absolute() or ".." in parts.parts or value == ".":
        raise ValueError(f"Path escapes or names the whole root: {value!r}")
    if not pattern and any(char in value for char in "*?["):
        raise ValueError(f"Concrete path required: {value!r}")
    return value


def inside(root, name):
    relative_name(name)
    root = Path(root).resolve()
    target = root / name
    if not target.resolve().is_relative_to(root):
        raise ValueError(f"Path escapes project through a link: {name}")
    cursor = target
    while cursor != root:
        if cursor.is_symlink() or (hasattr(cursor, "is_junction") and cursor.is_junction()):
            raise ValueError(f"Linked path is not supported: {name}")
        cursor = cursor.parent
    return target


def write_exclusive(path, data):
    with Path(path).open("xb") as stream:
        stream.write(data)


def workflow_name(root, name):
    """Resolve owned workflow paths without redirecting product snapshot paths."""
    relative_name(name)
    marker = inside(root, ISOLATED_BINDING)
    if marker.is_file():
        if inside(root, CLASSIC_BINDING).exists():
            raise ValueError("Two workflow-kit bindings found; reconcile them before continuing")
        binding = read_json(marker)
        if binding.get("package") != "workflow-kit" or binding.get("layout") != "isolated":
            raise ValueError("Unrecognized .workflow-kit binding; inspect integration before continuing")
        if name in {"tasks", "notes", "docs/workflow"} or name.startswith(("tasks/", "docs/workflow/", "notes/")) or name in {
            "scripts/project_workflow.py", "scripts/workflow_runtime.py", "scripts/workflow_bootstrap.py",
            "scripts/workflow_intake.py", "scripts/workflow_progress.py"
        }:
            return ISOLATED_ROOT + "/" + name
    elif inside(root, ISOLATED_ROOT).exists():
        raise ValueError("Incomplete or foreign .workflow-kit directory; run bootstrap diagnostics")
    return name


def workflow_inside(root, name):
    return inside(root, workflow_name(root, name))


def workflow_relative(root, name):
    """Logical owned name for inventory filters; legacy root paths stay distinct."""
    isolated = inside(root, ISOLATED_BINDING).is_file()
    if isolated:
        prefix = ISOLATED_ROOT + "/"
        return name[len(prefix):] if name.startswith(prefix) else None
    return name


def records(root, folder, prefix, errors):
    result = {}
    directory = workflow_inside(root, folder)
    if not directory.exists():
        return result
    for path in sorted(directory.glob("*.json")):
        try:
            inside(root, path.relative_to(root).as_posix())
            item = read_json(path)
            ident = item.get("id")
            if not isinstance(ident, str) or not ident.startswith(prefix) or path.stem != ident:
                raise ValueError("file name must equal id and use the expected prefix")
            if ident in result:
                raise ValueError("duplicate id")
            if item.get("schema_version") != VERSION:
                raise ValueError("unsupported schema_version")
            result[ident] = item
        except (ValueError, OSError) as error:
            errors.append(f"{path.relative_to(root)}: {error}")
    return result


def is_hash(value):
    return isinstance(value, str) and re.fullmatch(r"[a-fA-F0-9]{64}", value) is not None


def secret_kind(name):
    """'secret' must never be hashed; 'data' is skipped and recorded; None is ordinary."""
    lower = name.lower()
    if lower in SECRET_NAMES or lower.endswith(SECRET_SUFFIXES):
        return "secret"
    if lower.startswith(".env"):
        return "data" if lower.endswith((".example", ".sample", ".template", ".dist")) else "secret"
    if lower.endswith(DATA_SUFFIXES):
        return "data"
    return None


def capture(root, roots, allow_missing=False):
    """Hash explicitly selected trees; refuse secrets, record skipped data files and links."""
    root = Path(root).resolve()
    files, missing, excluded = {}, [], []
    roots = sorted(set(roots))
    if not roots:
        raise ValueError("At least one explicit snapshot path is required")
    for name in roots:
        target = inside(root, name)
        if not target.exists():
            if allow_missing:
                missing.append(name)
                continue
            raise ValueError(f"Snapshot input does not exist: {name}")
        pending = [target]
        while pending:
            path = pending.pop()
            rel = path.relative_to(root).as_posix()
            inside(root, rel)
            if any(part.lower() in DENIED_PARTS for part in path.relative_to(root).parts):
                if path != target:
                    continue
                raise ValueError(f"Excluded credential/dependency directory in snapshot: {rel}")
            kind = secret_kind(path.name)
            if kind == "secret":
                raise ValueError(f"Secret file excluded from snapshot; move it out of the selected paths: {rel}")
            if kind == "data":
                excluded.append(rel)
                continue
            if path.is_dir():
                pending.extend(sorted(path.iterdir(), reverse=True))
            elif path.is_file():
                files[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
            else:
                raise ValueError(f"Unsupported snapshot entry: {rel}")
    body = {"roots": roots, "files": [{"path": key, "sha256": files[key]} for key in sorted(files)]}
    if missing:
        body["missing"] = missing
    if excluded:
        body["excluded"] = sorted(excluded)
    if not files and not missing:
        raise ValueError("Snapshot must include at least one file")
    return {"schema_version": VERSION, "captured_at_utc": iso(utc_now()), **body, "digest": digest(body)}


def inspect_manifest(root, name, expected, current=False):
    manifest = read_json(inside(root, name))
    if manifest.get("schema_version") != VERSION:
        raise ValueError("unsupported snapshot version")
    body = {"roots": manifest["roots"], "files": manifest["files"]}
    for key in ("missing", "excluded"):
        if key in manifest:
            body[key] = manifest[key]
    if not is_hash(expected) or manifest.get("digest") != expected or digest(body) != expected:
        raise ValueError("snapshot digest does not match its content and task")
    if not isinstance(body["roots"], list) or not body["roots"] or not isinstance(body["files"], list) or (not body["files"] and not body.get("missing")):
        raise ValueError("empty or invalid snapshot")
    for name in body["roots"]:
        inside(root, name)
    for name in body.get("missing", []):
        inside(root, name)
        if name not in body["roots"]:
            raise ValueError("missing path must be an explicit snapshot root")
    paths = []
    for item in body["files"]:
        inside(root, item["path"])
        if not is_hash(item.get("sha256")):
            raise ValueError("invalid file hash")
        paths.append(item["path"])
    if len(set(paths)) != len(paths):
        raise ValueError("duplicate snapshot path")
    if current and capture(root, body["roots"], allow_missing=bool(body.get("missing")))["digest"] != expected:
        raise ValueError("candidate changed: files were added, removed or modified after verification")


def task_definition(task):
    """Freeze controlled fields, excluding mutable status, counters and evidence."""
    keys = ("id", "title", "kind", "objective", "non_goals", "requirement_refs", "dependencies", "risk", "scope", "acceptance", "gates")
    body = {key: task.get(key) for key in keys}
    for key in ("reference_ids", "decision_refs", "snapshot_paths", "continuation_of", "retry_safe", "ui_change", "ui_contract_ref", "ui_checks", "test_review"):
        if key in task:
            body[key] = task[key]
    return digest(body)


def task_deadline(task):
    extensions = task.get("budget", {}).get("extensions", [])
    return extensions[-1]["deadline_at_utc"] if extensions else task.get("budget", {}).get("deadline_at_utc")


def as_list(value):
    """Owner decisions sometimes record scope as one string; never iterate its characters."""
    if isinstance(value, str):
        return [value]
    return list(value) if isinstance(value, (list, tuple)) else []


def is_mutable_record(root, name):
    logical = workflow_relative(root, name)
    return logical is not None and (logical.startswith(MUTABLE_RECORDS) or logical in MUTABLE_FILES)


def gate_argv_problems(gate):
    """Reject argument shapes that are known to fail before any verification runs."""
    program = str(gate.get("program") or "")
    args = gate.get("args") if isinstance(gate.get("args"), list) else []
    stem = PurePosixPath(program.replace("\\", "/")).stem.lower()
    first = str(args[0]).lower() if args else ""
    if stem == "npm" and first in {"build", "lint", "dev", "start:dev", "typecheck", "check"}:
        return f"npm has no '{first}' command; use args [\"run\", \"{first}\"]"
    if stem in {"pnpm", "yarn"} and first == "build" and len(args) == 1:
        return f"{stem} build usually needs [\"run\", \"build\"] unless the project defines a build binary"
    if stem == "cargo" and first == "run" and len(args) > 1 and str(args[1]).lower() in {"test", "build", "check", "clippy"}:
        return f"cargo {args[1]} is a subcommand; use args [\"{args[1]}\"] instead of [\"run\", \"{args[1]}\"]"
    if stem in {"python", "python3", "{python}"} and first == "pytest":
        return "run pytest as a module: args [\"-m\", \"pytest\"]"
    return None


def repair_limit(task, policy):
    return policy["budget"]["max_repair_rounds"] + sum(
        entry.get("repair_rounds", 0) for entry in task.get("budget", {}).get("extensions", []))


def gate_command(gate):
    return (gate.get("program"), tuple(gate.get("args", [])), gate.get("cwd"))


def replaced_gates(task, dependency):
    """Exceptions are effective only when test_review_errors also accepts the plan."""
    review = task.get("test_review")
    actions = review.get("actions", []) if isinstance(review, dict) else []
    if not isinstance(actions, list):
        return set()
    return {item.get("from_gate") for item in actions if isinstance(item, dict)
            and item.get("from_task") == dependency and isinstance(item.get("from_gate"), str)
            and item.get("action") in {"adapt", "replace", "retire"}}


def test_review_errors(root, project, task, tasks, decisions):
    """Check the applicability record; semantic equivalence remains a review duty."""
    ident = task.get("id", "TASK")
    review = task.get("test_review")
    required = project.get("kind") == "refactor" and task.get("kind") not in {"documentation", "baseline"}
    if review is None:
        return [f"{ident}: refactor work needs test_review before implementation"] if required else []
    if not isinstance(review, dict):
        return [f"{ident}: test_review must be an object"]
    errors = []

    def require(condition, message):
        if not condition:
            errors.append(f"{ident}: test_review {message}")

    def nonempty(value):
        return isinstance(value, str) and bool(value.strip())

    require(review.get("behavior") in {"preserve", "change"}, "must distinguish preserved and changed behavior")
    baseline = review.get("baseline")
    if not isinstance(baseline, dict):
        errors.append(f"{ident}: test_review needs the original baseline and its disposition")
    else:
        require(baseline.get("status") in {"PASS", "FAIL", "NOT_RUN", "BLOCKED"}, "baseline status must be explicit")
        require(nonempty(baseline.get("summary")), "baseline needs a factual summary, including known failures/limits")
        try:
            path = inside(root, baseline.get("evidence_ref"))
            require(path.is_file() and path.stat().st_size > 0, "baseline needs an existing, nonempty evidence file")
        except (ValueError, OSError, TypeError) as error:
            errors.append(f"{ident}: test_review baseline evidence: {error}")
    ids = review.get("decision_ids", [])
    valid_ids = isinstance(ids, list) and all(isinstance(value, str) for value in ids)
    require(valid_ids, "decision_ids must be a list")
    decisions_used = [decisions.get(value, {}) for value in ids] if valid_ids else []
    require(all(item.get("status") == "accepted" and item.get("issuer") == "owner" and nonempty(item.get("source"))
                for item in decisions_used), "decision_ids must reference accepted owner decisions")
    coverage = set(task.get("requirement_refs", [])) | {ident}
    scoped_choice = bool(decisions_used) and any(coverage.intersection(as_list(item.get("scope"))) for item in decisions_used)
    if review.get("behavior") == "change":
        require(scoped_choice, "behavior change needs an owner decision whose scope lists this task or one of its requirement_refs; "
                "task coverage is " + json.dumps(sorted(coverage), ensure_ascii=False) + ", decision scopes are "
                + json.dumps([as_list(item.get("scope")) for item in decisions_used], ensure_ascii=False))
    required_ids = {gate.get("id") for gate in task.get("gates", []) if isinstance(gate, dict) and gate.get("required")}
    require(bool(required_ids), "must retain an executable required verification gate")
    actions = review.get("actions")
    if not isinstance(actions, list) or not actions:
        return errors + [f"{ident}: test_review needs grouped keep/adapt/add/replace/retire actions"]
    seen = set()
    for item in actions:
        if not isinstance(item, dict):
            errors.append(f"{ident}: test_review actions must be objects")
            continue
        action = item.get("action")
        require(action in {"keep", "adapt", "add", "replace", "retire"}, "has an unknown test disposition")
        require(nonempty(item.get("target")) and nonempty(item.get("reason")), "actions need the affected checks and a substantive reason")
        gate_ids = item.get("gate_ids")
        valid_gates = isinstance(gate_ids, list) and all(isinstance(value, str) for value in gate_ids)
        require(valid_gates, "gate_ids must be a list")
        if action == "retire":
            require(gate_ids == [], "retired requirements use empty gate_ids, not fake replacements")
        else:
            require(valid_gates and bool(gate_ids) and all(value in required_ids for value in gate_ids),
                    "kept/adapted/new/replacement checks must map to required gates")
        if action in {"replace", "retire"}:
            require(review.get("behavior") == "change" and scoped_choice,
                    "replacing/retiring a behavioral contract requires the corresponding owner choice")
        dependency, gate_id = item.get("from_task"), item.get("from_gate")
        if dependency is not None or gate_id is not None:
            if not isinstance(dependency, str) or not isinstance(gate_id, str):
                errors.append(f"{ident}: test_review from_task/from_gate must identify an exact prior gate")
                continue
            prior = tasks.get(dependency, {})
            require(dependency in task.get("dependencies", []) and prior.get("status") in {"verified", "done"},
                    "gate transition must reference an available direct dependency")
            require(any(gate.get("id") == gate_id and gate.get("required") for gate in prior.get("gates", [])),
                    "gate transition references a missing or optional prior gate")
            require(action in {"keep", "adapt", "replace", "retire"}, "new coverage cannot suppress a prior gate")
            require((dependency, gate_id) not in seen, "has duplicate dispositions for a prior gate")
            seen.add((dependency, gate_id))
    return errors


def recovery_policy(policy):
    return {**RECOVERY_DEFAULTS, **policy.get("recovery", {})}


def required_review_mode(policy, task):
    if task.get("risk", {}).get("level") == "high":
        return "independent_required"
    return policy.get("review", {}).get("mode")


def review_quality_digest(root, task, report):
    """Bind a substantive review to the verified candidate and immutable evidence.

    The reference surface is closed: evidence is either a file of the verified
    candidate (hash taken from the candidate manifest, so later tasks may change
    the source) or an immutable run/evidence attachment. Records the tool itself
    rewrites (task items, cards, decisions, notes) would make the review
    invalidate itself and are rejected with an explicit reason.
    """
    checks = report.get("review_checks")
    kind = task.get("kind")
    required_areas = {"requirements", "regression"} if kind in {"documentation", "baseline"} else set(REVIEW_AREAS)
    areas = [item.get("area") for item in checks if isinstance(item, dict)] if isinstance(checks, list) else []
    if (not isinstance(checks, list) or not required_areas <= set(areas) or not set(areas) <= set(REVIEW_AREAS)
            or len(checks) != len(areas) or len(set(areas)) != len(areas)):
        raise ValueError("Review needs one evidence-based check per area: " + ", ".join(sorted(required_areas))
                         + (" (failure_paths, maintainability, performance are optional for documentation/baseline tasks)" if kind in {"documentation", "baseline"} else ""))
    evidence = task.get("evidence", {})
    if report.get("verification_run") != evidence.get("verification_run"):
        raise ValueError("Review must identify the actual current verification_run")
    snapshot = read_json(inside(root, evidence["candidate_manifest"]))
    candidate_files = {item["path"]: item["sha256"] for item in snapshot["files"]}
    attachments = tuple(workflow_name(root, prefix) for prefix in ("tasks/evidence/", "tasks/runs/"))
    recorded = {}
    for check in checks:
        if not isinstance(check, dict) or check.get("status") not in {"PASS", "NOT_APPLICABLE"}:
            raise ValueError("Passing review cannot include unresolved or failed review checks")
        if not isinstance(check.get("analysis"), str) or not check["analysis"].strip():
            raise ValueError("Review checks need an explanation of what was checked and why it supports the conclusion")
        if check["status"] == "NOT_APPLICABLE" and (check["area"] in {"requirements", "regression"}
                or (check["area"] == "failure_paths" and task.get("risk", {}).get("level") == "high")):
            raise ValueError("Required review coverage cannot be marked NOT_APPLICABLE")
        files = check.get("evidence_files")
        if not isinstance(files, list) or not all(isinstance(name, str) for name in files) or (check["status"] == "PASS" and not files):
            raise ValueError("Passing review checks need actual evidence_files; inapplicable checks still need an explicit list")
        for name in files:
            if name in recorded:
                continue
            inside(root, name)
            if name in candidate_files:
                recorded[name] = candidate_files[name]
                continue
            if is_mutable_record(root, name):
                raise ValueError("Review evidence cannot cite a record the tool rewrites (task items, cards, decisions, notes): " + name
                                 + "; cite candidate files, tasks/runs/ or tasks/evidence/ instead")
            if not name.startswith(attachments):
                raise ValueError("Review evidence must be a file of the verified candidate or an attachment under tasks/evidence/ or tasks/runs/: "
                                 + name + "; add other sources to the task's snapshot_paths before verification")
            path = inside(root, name)
            if not path.is_file() or path.stat().st_size == 0:
                raise ValueError("Review evidence is missing/empty: " + name)
            recorded[name] = hashlib.sha256(path.read_bytes()).hexdigest()
    verification_name = workflow_name(root, "tasks/runs/" + report["verification_run"] + ".json")
    if verification_name not in recorded:
        verification = inside(root, verification_name)
        recorded[verification_name] = hashlib.sha256(verification.read_bytes()).hexdigest()
    return digest({"report": report, "candidate": evidence["candidate_digest"], "evidence": recorded})


def ui_preview_accepted(root, policy, project, tasks):
    if policy.get("ui", {}).get("mode", "none") != "preview_first":
        return True
    preview = tasks.get(project.get("ui_preview_task"), {})
    evidence = preview.get("evidence", {})
    accepted = (preview.get("kind") == "ui_preview" and preview.get("status") == "done"
                and bool(evidence.get("acceptance_ref")) and bool(evidence.get("owner_decision_ids")))
    if not accepted:
        return False
    try:
        contract = preview["ui_contract_ref"]
        manifest = read_json(inside(root, evidence["candidate_manifest"]))
        recorded = next(item["sha256"] for item in manifest["files"] if item["path"] == contract)
        return hashlib.sha256(inside(root, contract).read_bytes()).hexdigest() == recorded
    except (ValueError, OSError, KeyError, TypeError, StopIteration):
        return False


def ui_check_ids(task):
    """ui_checks entries are either "select.open" strings or {"id": ..., "description": ...} objects."""
    ids = []
    for item in task.get("ui_checks", []) or []:
        if isinstance(item, str):
            ids.append(item.strip())
        elif isinstance(item, dict) and isinstance(item.get("id"), str):
            ids.append(item["id"].strip())
        else:
            ids.append("")
    return ids


def ui_review_digest(root, task, report):
    """Bind declared visual/interaction evidence; aesthetic judgment stays human."""
    if not task.get("ui_change", False):
        return None
    review = report.get("ui_review")
    if not isinstance(review, dict) or review.get("contract_ref") != task.get("ui_contract_ref"):
        raise ValueError("UI review must reference the task's frozen UI contract")
    states, files = review.get("checked_states"), review.get("evidence_files")
    if not isinstance(states, list) or not all(isinstance(value, str) for value in states):
        raise ValueError("UI review checked_states must list the ids of the checked ui_checks")
    declared = set(ui_check_ids(task))
    checked = {value.strip() for value in states}
    if not declared <= checked:
        raise ValueError("UI review must cover every declared ui_check id; missing: " + ", ".join(sorted(declared - checked)))
    if not isinstance(files, list) or not files or not all(isinstance(value, str) for value in files) or len(set(files)) != len(files):
        raise ValueError("UI review needs distinct screenshot and interaction-report files")
    suffixes, evidence = set(), []
    for name in sorted(files):
        path = inside(root, name)
        if not name.startswith(workflow_name(root, "tasks/evidence/")) or not path.is_file() or path.stat().st_size == 0:
            raise ValueError("UI evidence must be a nonempty file under tasks/evidence/: " + name)
        suffixes.add(path.suffix.lower())
        evidence.append({"path": name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
    if not suffixes.intersection({".png", ".jpg", ".jpeg", ".webp"}) or not suffixes.intersection({".md", ".json", ".txt"}):
        raise ValueError("UI review requires a screenshot and an interaction report, not only a build log")
    return digest({"contract_ref": review["contract_ref"], "checked_states": sorted(states), "files": evidence})


def budget_clock(policy):
    """'active' counts only writer runs (implementation/repair/probe); 'wall' is the legacy deadline."""
    return policy.get("budget", {}).get("clock", "wall")


def allowance_seconds(task, policy):
    minutes = policy.get("budget", {}).get("max_task_wall_minutes", 0)
    extra = sum(entry.get("minutes", 0) for entry in task.get("budget", {}).get("extensions", []) if type(entry.get("minutes")) is int)
    return (minutes + extra) * 60


def consumed_seconds(task, runs, now):
    total = 0.0
    for run in runs:
        if run.get("kind") not in {"implementation", "repair", "probe"}:
            continue
        try:
            start = datetime.fromisoformat(str(run.get("started_at_utc")).replace("Z", "+00:00"))
            finish = datetime.fromisoformat(str(run.get("finished_at_utc")).replace("Z", "+00:00")) if run.get("finished_at_utc") else now
        except ValueError:
            continue
        total += max(0.0, (finish - start).total_seconds())
    return total


def committed_evidence_problem(root, name):
    """Optional strict gate: acceptance evidence must be tracked by git with no pending changes."""
    import shutil
    import subprocess
    git = shutil.which("git")
    if not git:
        return "git is not available for the strict acceptance check"
    try:
        tracked = subprocess.run([git, "-C", str(root), "ls-files", "--error-unmatch", "--", name], capture_output=True, timeout=20)
        if tracked.returncode != 0:
            return "acceptance evidence is not tracked by git: " + name
        pending = subprocess.run([git, "-C", str(root), "status", "--porcelain", "--", name], capture_output=True, text=True, encoding="utf-8", timeout=20)
        if pending.stdout.strip():
            return "acceptance evidence has uncommitted changes: " + name
    except (OSError, subprocess.SubprocessError) as error:
        return "strict acceptance check failed: " + str(error)
    return None


def check_project(root, now=None):
    root = Path(root).resolve()
    now = now or utc_now()
    errors, warnings = [], []
    from workflow_bootstrap import integration_status
    integration = integration_status(root)
    if not integration["connected"]:
        return {"ok": False, "errors": integration["errors"] or ["workflow-kit is not connected; run bootstrap first"],
                "warnings": [], "counts": {}, "integration": integration}

    def require(condition, message):
        if not condition:
            errors.append(message)

    def time_value(value, label, required=True):
        if value is None and not required:
            return None
        try:
            if not isinstance(value, str):
                raise ValueError("time missing")
            parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                raise ValueError("timezone missing")
            return parsed
        except ValueError:
            errors.append(f"{label}: expected ISO time with timezone")
            return None

    def local_ref(value, label):
        try:
            require(inside(root, value).is_file(), f"{label}: missing file {value}")
        except (ValueError, OSError) as error:
            errors.append(f"{label}: {error}")

    try:
        project = read_json(workflow_inside(root, "tasks/PROJECT.json"))
        policy = read_json(workflow_inside(root, project.get("policy", "tasks/POLICY.json")))
        decisions_file = read_json(workflow_inside(root, "tasks/DECISIONS.json"))
    except (ValueError, OSError) as error:
        return {"ok": False, "errors": [str(error)], "warnings": [], "counts": {}}

    for name, item in (("PROJECT", project), ("POLICY", policy), ("DECISIONS", decisions_file)):
        require(item.get("schema_version") == VERSION, f"{name}: unsupported schema_version")
    require(project.get("kind") in {"new", "refactor"}, "PROJECT: choose new or refactor")
    require(project.get("stage") in {"intake", "discovery", "baseline", "delivery", "release", "complete", "paused"}, "PROJECT: unknown stage")
    local_ref(project.get("handoff"), "PROJECT.handoff")
    decisions = {}
    for decision in decisions_file.get("decisions", []):
        if not isinstance(decision, dict):
            errors.append("DECISIONS: entries must be objects")
            continue
        ident = decision.get("id")
        require(isinstance(ident, str) and ident not in decisions, "DECISIONS: missing or duplicate id")
        if isinstance(ident, str):
            decisions[ident] = decision
        if decision.get("status") == "accepted":
            require(decision.get("issuer") == "owner" and bool(decision.get("statement")) and bool(decision.get("scope")) and bool(decision.get("source")), f"{ident}: accepted decision requires owner, statement, scope and source")
            stamp = time_value(decision.get("recorded_at_utc"), f"{ident}.recorded_at_utc")
            if stamp:
                require(stamp <= now + timedelta(seconds=60), f"{ident}: recorded time is in the future")

    def approved_ids(ids):
        return isinstance(ids, list) and bool(ids) and all(decisions.get(ident, {}).get("status") == "accepted" and decisions[ident].get("issuer") == "owner" for ident in ids if isinstance(ident, str)) and all(isinstance(ident, str) for ident in ids)

    approval = policy.get("approval", {})
    approved = approval.get("status") == "approved" and approved_ids(approval.get("decision_ids"))
    require(approval.get("status") in {"pending", "approved"}, "POLICY: unknown approval status")
    if approval.get("status") == "approved":
        require(approved, "POLICY: approval requires existing accepted owner decisions")
        if policy.get("intake", {}).get("version") == 1:
            from workflow_intake import audit as audit_intake
            intake_id = policy["intake"].get("decision_id")
            brief = decisions.get(intake_id, {}).get("confirmed_brief")
            require(intake_id in approval.get("decision_ids", []), "POLICY: intake decision must remain in approval references")
            require(isinstance(brief, dict), "POLICY: approved intake needs its confirmed brief and answer sources")
            if isinstance(brief, dict):
                interview = audit_intake(brief, project.get("kind"))
                require(interview["ready_for_onboard"], "POLICY: incomplete or stale intake confirmations: " + ", ".join(interview["missing"]))
                try:
                    current_brief = read_json(workflow_inside(root, "tasks/BRIEF.json"))
                    facts = lambda item: {key: value for key, value in item.items() if key not in {"schema_version", "approval_source"}}
                    require(facts(current_brief) == facts(brief), "BRIEF: confirmed scope changed without updating its owner decision")
                except (ValueError, OSError) as error:
                    errors.append("BRIEF: missing confirmed scope: " + str(error))
                selected = brief.get("execution", {})
                require(policy.get("execution", {}).get("mode") == selected.get("mode")
                        and all(policy.get("roles", {}).get(role, {}).get("harness") == selected.get(role) for role in ("coder", "reviewer")),
                        "POLICY: execution choice changed; record a new confirmed decision before switching Agent/CLI")
                require(not policy.get("authority", {}).get("code") or brief.get("authority", {}).get("code") is True,
                        "POLICY: implementation needs a confirmed code choice; assessment approval cannot authorize it")
                require(policy.get("review", {}).get("mode") == selected.get("review_mode"),
                        "POLICY: review choice changed; preserve the confirmed independence requirement")
        else:
            warnings.append("Legacy approval has no intake audit; preserve its history and verify the owner's execution and scope choices before expanding work.")
    else:
        warnings.append("Onboarding is pending; this check does not authorize application work.")

    limits = policy.get("budget", {})
    if approved:
        for key in ("max_repair_rounds", "max_task_wall_minutes", "max_tasks_per_batch", "max_concurrent_workers"):
            value = limits.get(key)
            require(type(value) is int and value >= (0 if key == "max_repair_rounds" else 1), f"POLICY: explicit valid budget required for {key}")
        for role in ("manager", "coder", "reviewer"):
            selected = policy.get("roles", {}).get(role, {})
            require(bool(selected.get("agent")) and bool(selected.get("harness")), f"POLICY: select {role} agent/harness")
        require(policy.get("review", {}).get("mode") in {"independent_required", "self_review_allowed"}, "POLICY: select explicit review mode")
        require(policy.get("review", {}).get("evidence_version") == 1, "POLICY: review evidence requirements cannot be disabled; migrate older records explicitly")
        require(policy.get("review", {}).get("high_risk") == "independent_required", "POLICY: high-risk review must remain independent")
        require(limits.get("batch_rollover", "ask") in {"ask", "allowed"}, "POLICY: batch_rollover must be ask or allowed")
        require(limits.get("clock", "wall") in {"wall", "active"}, "POLICY: budget.clock must be wall or active")
        require(type(policy.get("acceptance", {}).get("require_committed_evidence", False)) is bool,
                "POLICY: acceptance.require_committed_evidence must be a boolean")
        recovery = recovery_policy(policy)
        delays = recovery["network_backoff_seconds"]
        require(isinstance(delays, list) and len(delays) <= 3 and all(type(value) is int and 0 <= value <= 60 for value in delays),
                "POLICY: network backoff must contain at most three delays of 0..60 seconds")
        require(type(recovery["max_cli_call_seconds"]) is int and 1 <= recovery["max_cli_call_seconds"] <= 3600,
                "POLICY: max_cli_call_seconds must be 1..3600")
        finance = limits.get("financial", {})
        require(finance.get("mode") in {"none", "limit"}, "POLICY: cost mode must be explicitly none or limit")
        if finance.get("mode") == "limit":
            require(type(finance.get("amount_usd")) in {int, float} and math.isfinite(finance["amount_usd"]) and finance["amount_usd"] > 0, "POLICY: positive cost limit required")
        if finance.get("mode") == "none":
            require(finance.get("amount_usd") is None, "POLICY: none cost mode must not contain an amount")
    authority = policy.get("authority", {})
    require(policy.get("ui", {}).get("mode", "none") in {"none", "existing", "preview_first"}, "POLICY: unknown UI mode")
    for key in ("commit", "push", "pull_request", "merge", "release"):
        require(authority.get(key) in {"ask", "allowed", "forbidden"}, f"POLICY: invalid {key} authority")

    tasks = records(root, "tasks/items", "TASK-", errors)
    batches = records(root, "tasks/batches", "BATCH-", errors)
    runs = records(root, "tasks/runs", "RUN-", errors)
    require(project.get("current_task") is None or project["current_task"] in tasks, "PROJECT: current_task does not exist")
    require(project.get("current_batch") is None or project["current_batch"] in batches, "PROJECT: current_batch does not exist")
    if project.get("current_task") in tasks:
        require(tasks[project["current_task"]].get("status") in ACTIVE | {"blocked"}, "PROJECT: current_task points to an inactive task")
    preview_id = project.get("ui_preview_task")
    require(preview_id is None or tasks.get(preview_id, {}).get("kind") == "ui_preview", "PROJECT: UI preview task is missing or has the wrong kind")

    by_task = {ident: [] for ident in tasks}
    for ident, run in runs.items():
        task_id = run.get("task_id")
        require(task_id in tasks, f"{ident}: unknown task_id")
        if task_id in by_task:
            by_task[task_id].append(run)
        require(run.get("kind") in RUN_KINDS, f"{ident}: unknown run kind")
        require(run.get("outcome") in {"running", "completed", "failed", "blocked", "cancelled"}, f"{ident}: unknown outcome")
        require(bool(run.get("executor", {}).get("harness")), f"{ident}: executor harness missing")
        start = time_value(run.get("started_at_utc"), f"{ident}.started_at_utc")
        finish = time_value(run.get("finished_at_utc"), f"{ident}.finished_at_utc", run.get("outcome") != "running")
        if start:
            require(start <= now + timedelta(seconds=60), f"{ident}: start is in the future")
        if finish:
            require(finish <= now + timedelta(seconds=60), f"{ident}: finish is in the future")
            if start:
                require(finish >= start, f"{ident}: finish precedes start")
        for ref in run.get("raw_output_refs", []):
            local_ref(ref, ident)
        cost = run.get("estimated_cost_usd")
        require(cost is None or (type(cost) in {int, float} and math.isfinite(cost) and cost >= 0), f"{ident}: invalid estimated cost")

    for ident, batch in batches.items():
        ids = batch.get("task_ids", [])
        require(isinstance(ids, list) and len(set(ids)) == len(ids), f"{ident}: duplicate or invalid task_ids")
        require(batch.get("status") in {"planned", "active", "closed"}, f"{ident}: unknown batch status")
        maximum = limits.get("max_tasks_per_batch")
        if type(maximum) is int:
            require(len(ids) <= maximum, f"{ident}: batch task limit exceeded")
        for task_id in ids:
            require(task_id in tasks and tasks[task_id].get("batch_id") == ident, f"{ident}: task reference is not bidirectional: {task_id}")
        if batch.get("status") == "active":
            require(approved and approved_ids(batch.get("approval_decision_ids")), f"{ident}: active batch lacks approval reference")
        finance = limits.get("financial", {})
        if finance.get("mode") == "limit" and type(finance.get("amount_usd")) in {int, float}:
            known_cost = sum(run.get("estimated_cost_usd") or 0 for run in runs.values() if run.get("task_id") in ids and type(run.get("estimated_cost_usd")) in {int, float})
            require(known_cost <= finance["amount_usd"], f"{ident}: recorded estimated batch cost exceeds limit")

    visiting, visited = set(), set()

    def visit(task_id):
        if task_id in visiting:
            errors.append(f"{task_id}: dependency cycle")
            return
        if task_id in visited:
            return
        visiting.add(task_id)
        for dependency in tasks[task_id].get("dependencies", []):
            if dependency in tasks:
                visit(dependency)
        visiting.remove(task_id)
        visited.add(task_id)

    for task_id in tasks:
        visit(task_id)
    writers = [run for run in runs.values() if run.get("kind") in {"implementation", "repair", "probe"} and run.get("outcome") == "running"]
    if type(limits.get("max_concurrent_workers")) is int:
        require(len(writers) <= limits["max_concurrent_workers"], "POLICY: concurrent worker limit exceeded")
    for task_id in tasks:
        require(sum(run.get("task_id") == task_id for run in writers) <= 1, f"{task_id}: multiple active writers")

    for ident, task in tasks.items():
        status = task.get("status")
        require(status in TASK_STATES, f"{ident}: unknown task state")
        require(isinstance(task.get("blockers"), list), f"{ident}: blockers must be a list")
        require(type(task.get("retry_safe", False)) is bool, f"{ident}: retry_safe must be a boolean")
        require(type(task.get("ui_change", False)) is bool, f"{ident}: ui_change must be a boolean")
        if task.get("kind") == "ui_preview":
            require(task.get("ui_change") is True, f"{ident}: UI preview must declare a UI change")
        if task.get("ui_change"):
            local_ref(task.get("ui_contract_ref"), f"{ident} UI contract")
            ids = ui_check_ids(task)
            require(isinstance(task.get("ui_checks"), list) and bool(ids) and all(ids) and len(set(ids)) == len(ids),
                    f"{ident}: UI changes need explicit component/interaction checks with unique ids")
        if status in PREPARED | {"blocked"}:
            errors += test_review_errors(root, project, task, tasks, decisions)
        if status == "blocked":
            require(bool(task.get("blockers")), f"{ident}: blocked requires a reason")
        if status in PREPARED:
            require(not task.get("blockers"), f"{ident}: {status} contradicts blockers")
        related = by_task[ident]
        listed = task.get("run_ids", [])
        require(isinstance(listed, list) and len(set(listed)) == len(listed) and set(listed) == {run["id"] for run in related}, f"{ident}: run_ids omit, duplicate or misattribute historical runs")
        batch_id = task.get("batch_id")
        if batch_id:
            require(batch_id in batches and ident in batches[batch_id].get("task_ids", []), f"{ident}: missing bidirectional batch membership")
        budget = task.get("budget", {})
        repairs = sum(run.get("kind") == "repair" for run in related)
        require(type(budget.get("repair_rounds_used")) is int and budget["repair_rounds_used"] == repairs, f"{ident}: repair counter differs from run history")
        started = time_value(budget.get("started_at_utc"), f"{ident}.budget.started_at_utc", bool(related) or status in {"running", "verifying", "review", "verified", "done"})
        deadline = time_value(budget.get("deadline_at_utc"), f"{ident}.budget.deadline_at_utc", started is not None)
        original_deadline = deadline
        extra_repairs = 0
        for extension in budget.get("extensions", []):
            decision = decisions.get(extension.get("decision_id"), {})
            require(approved_ids([extension.get("decision_id")]) and ident in as_list(decision.get("scope"))
                    and decision.get("budget_extension") == extension,
                    f"{ident}: budget extension requires its exact accepted owner decision")
            previous = time_value(extension.get("previous_deadline_at_utc"), f"{ident}.extension.previous")
            recorded = time_value(extension.get("recorded_at_utc"), f"{ident}.extension.recorded")
            extended = time_value(extension.get("deadline_at_utc"), f"{ident}.extension.deadline")
            minutes, rounds = extension.get("minutes"), extension.get("repair_rounds")
            valid_amount = type(minutes) is int and minutes > 0 and type(rounds) is int and rounds >= 0
            require(valid_amount, f"{ident}: extension needs positive minutes and nonnegative repair rounds")
            if previous and recorded and extended and valid_amount:
                require(previous == deadline and recorded <= now + timedelta(seconds=60)
                        and extended == max(previous, recorded) + timedelta(minutes=minutes),
                        f"{ident}: invalid budget extension chain")
                deadline = extended
                extra_repairs += rounds
        if type(limits.get("max_repair_rounds")) is int:
            require(repairs <= limits["max_repair_rounds"] + extra_repairs, f"{ident}: repair limit exceeded")
        if started and deadline:
            require(deadline > started, f"{ident}: deadline must follow original start")
            # Only writing phases consume the clock. Verification and review are
            # read-only gates and may still finish after the deadline.
            if status in {"ready", "running"}:
                if budget_clock(policy) == "active":
                    require(consumed_seconds(task, related, now) < allowance_seconds(task, policy),
                            f"{ident}: active-time allowance exhausted; record blocked or extend instead of continuing")
                else:
                    require(now < deadline, f"{ident}: deadline exhausted; record blocked instead of continuing")
            for run in related:
                run_start = time_value(run.get("started_at_utc"), run["id"])
                run_finish = time_value(run.get("finished_at_utc"), run["id"], False)
                if run_start:
                    require(run_start >= started, f"{ident}: original task start was reset after an earlier run")
                if run_finish and status in PREPARED and run.get("kind") in {"implementation", "repair"} and budget_clock(policy) == "wall":
                    require(run_finish <= deadline, f"{ident}: run completed after task deadline")
            if status in PREPARED and type(limits.get("max_task_wall_minutes")) is int:
                require(original_deadline <= started + timedelta(minutes=limits["max_task_wall_minutes"]), f"{ident}: deadline exceeds configured wall budget")

        if status not in PREPARED:
            continue
        require(approved, f"{ident}: prepared task requires approved policy")
        if status in ACTIVE:
            require(project.get("stage") not in {"intake", "paused", "complete"}, f"{ident}: project stage does not permit active work")
            require(batch_id in batches and batches[batch_id].get("status") == "active", f"{ident}: task needs an active batch")
            if task.get("kind") not in {"ui_preview", "documentation", "baseline"}:
                require(ui_preview_accepted(root, policy, project, tasks), f"{ident}: accept the UI preview before production implementation")
        permission = {"documentation": "documentation", "baseline": "baseline", "ci": "ci"}.get(task.get("kind"), "code")
        require(authority.get(permission) is True, f"{ident}: task kind lacks {permission} authority")
        require(bool(task.get("title")) and bool(task.get("objective")) and bool(task.get("requirement_refs")) and bool(task.get("acceptance")), f"{ident}: incomplete objective/requirements/acceptance")
        level = task.get("risk", {}).get("level")
        require(level in {"low", "medium", "high"}, f"{ident}: risk not assessed")
        if level == "high":
            require(approved_ids(task.get("risk", {}).get("decision_ids")), f"{ident}: high risk requires accepted owner decision references")
        for dep in task.get("dependencies", []):
            require(dep != ident and dep in tasks and tasks[dep].get("status") in {"verified", "done"}, f"{ident}: dependency not available: {dep}")
        scope = task.get("scope", {})
        require(bool(scope.get("allowed_paths")), f"{ident}: empty allowed_paths")
        for name in scope.get("allowed_paths", []) + scope.get("protected_paths", []):
            try:
                relative_name(name, pattern=True)
            except ValueError as error:
                errors.append(f"{ident}: {error}")
        inputs = task.get("input", {})
        try:
            inspect_manifest(root, inputs.get("manifest"), inputs.get("digest"), current=status == "ready" and not related)
        except (ValueError, OSError, KeyError, TypeError) as error:
            errors.append(f"{ident}: invalid input snapshot: {error}")
        require(inputs.get("definition_digest") == task_definition(task), f"{ident}: controlled task definition differs from frozen digest")
        gates = task.get("gates", [])
        require(bool(gates), f"{ident}: no verification plan")
        gate_ids = []
        for gate in gates:
            gate_ids.append(gate.get("id"))
            require(bool(gate.get("id")) and isinstance(gate.get("required"), bool), f"{ident}: gate id/required missing")
            if gate.get("required"):
                require(bool(gate.get("program")) and isinstance(gate.get("args"), list) and bool(gate.get("cwd")), f"{ident}: required gate has no concrete command")
            else:
                require(bool(gate.get("reason")), f"{ident}: optional gate needs a pre-dispatch reason")
        require(len(set(gate_ids)) == len(gate_ids), f"{ident}: duplicate gate ids")

        if status not in {"verified", "done"}:
            continue
        evidence = task.get("evidence", {})
        candidate = evidence.get("candidate_digest")
        successor_id = evidence.get("continued_by")
        if successor_id:
            successor = tasks.get(successor_id, {})
            require(successor.get("status") in PREPARED | {"blocked"}
                    and ident in successor.get("dependencies", [])
                    and successor.get("continuation_of", {}).get(ident) == candidate,
                    f"{ident}: invalid candidate continuation: {successor_id}")
            replaced = replaced_gates(successor, ident)
            required_commands = {gate_command(gate) for gate in gates if gate.get("required") and gate.get("id") not in replaced}
            successor_commands = {gate_command(gate) for gate in successor.get("gates", []) if gate.get("required")}
            require(required_commands <= successor_commands, f"{ident}: continuation dropped required regression checks")
            try:
                previous_roots = read_json(inside(root, evidence["candidate_manifest"]))["roots"]
                require(set(previous_roots) <= set(successor.get("snapshot_paths", [])),
                        f"{ident}: continuation does not cover previous candidate roots")
            except (ValueError, OSError, KeyError, TypeError) as error:
                errors.append(f"{ident}: invalid continuation snapshot: {error}")
        try:
            inspect_manifest(root, evidence.get("candidate_manifest"), candidate, current=status == "verified" and not successor_id)
        except (ValueError, OSError, KeyError, TypeError) as error:
            errors.append(f"{ident}: invalid candidate snapshot: {error}")
        verification = runs.get(evidence.get("verification_run"), {})
        review = runs.get(evidence.get("review_run"), {})
        for kind, run in (("verification", verification), ("review", review)):
            require(run.get("task_id") == ident and run.get("kind") == kind and run.get("outcome") == "completed" and run.get("candidate_digest") == candidate, f"{ident}: invalid or stale {kind} evidence")
        checks = verification.get("checks", [])
        check_ids = [item.get("gate_id") for item in checks]
        require(len(set(check_ids)) == len(check_ids), f"{ident}: duplicate gate results")
        for item in checks:
            require(item.get("gate_id") in gate_ids and item.get("status") in GATE_STATES, f"{ident}: unknown gate result")
        checks_by_id = {item.get("gate_id"): item for item in checks}
        for gate in gates:
            if gate.get("required"):
                item = checks_by_id.get(gate.get("id"), {})
                require(item.get("status") == "PASS" and type(item.get("exit_code")) is int and item["exit_code"] == 0 and item.get("candidate_digest") == candidate, f"{ident}: required gate lacks current PASS: {gate.get('id')}")
                local_ref(item.get("log"), f"{ident} gate {gate.get('id')}")
        review_result = review.get("review", {})
        require(review_result.get("verdict") == "PASS" and review_result.get("candidate_digest") == candidate, f"{ident}: review did not pass current candidate")
        local_ref(review_result.get("report"), f"{ident} review")
        if policy.get("review", {}).get("evidence_version") == 1:
            try:
                quality_digest = review_quality_digest(root, task, read_json(inside(root, review_result["report"])))
                require(review_result.get("quality_digest") == quality_digest,
                        f"{ident}: review evidence changed after approval (if only the tool changed, run recompute --task {ident} --source ...)")
            except (ValueError, OSError, KeyError, TypeError) as error:
                errors.append(f"{ident}: invalid review evidence: {error}")
        if task.get("ui_change"):
            try:
                ui_digest = ui_review_digest(root, task, read_json(inside(root, review_result["report"])))
                require(review_result.get("ui_evidence_digest") == ui_digest, f"{ident}: UI evidence changed after review")
            except (ValueError, OSError, KeyError, TypeError) as error:
                errors.append(f"{ident}: invalid UI evidence: {error}")
        mode = review_result.get("mode")
        require(mode in {"independent", "self_review"}, f"{ident}: review mode missing")
        if required_review_mode(policy, task) == "independent_required":
            require(mode == "independent", f"{ident}: self review cannot satisfy independent_required")
        if mode == "independent":
            context = review.get("executor", {}).get("context_id")
            writer_contexts = {run.get("executor", {}).get("context_id") for run in related if run.get("kind") in {"implementation", "repair"}}
            require(bool(context) and context not in writer_contexts, f"{ident}: independent review shares or lacks a distinct context id")
        verification_start = time_value(verification.get("started_at_utc"), f"{ident}.verification.start")
        for run in related:
            require(run.get("outcome") != "running", f"{ident}: verified task still has an active run")
            if run.get("kind") in {"implementation", "repair"}:
                finished = time_value(run.get("finished_at_utc"), run["id"], False)
                if finished and verification_start:
                    require(finished <= verification_start, f"{ident}: verification precedes later implementation")
        if status == "done":
            local_ref(evidence.get("acceptance_ref"), f"{ident} acceptance")
            require(approved_ids(evidence.get("owner_decision_ids")), f"{ident}: done lacks owner acceptance reference")
            require(bool(evidence.get("merge_ref")), f"{ident}: done needs merge evidence or explicit not_applicable explanation")
            if policy.get("acceptance", {}).get("require_committed_evidence") is True and isinstance(evidence.get("acceptance_ref"), str):
                problem = committed_evidence_problem(root, evidence["acceptance_ref"])
                require(problem is None, f"{ident}: {problem}")

    return {"ok": not errors, "errors": errors, "warnings": warnings, "counts": {"tasks": len(tasks), "runs": len(runs), "batches": len(batches)}}


def error_owner(message):
    """Task-attributed errors start with the task id; everything else is global."""
    match = re.match(r"(TASK-[A-Za-z0-9_-]+)(?=[\s:.])", message)
    return match.group(1) if match else None


def relevant_errors(errors, task_ids=(), baseline=()):
    """Errors that should stop the current operation.

    Global errors always count. Errors attributed to other tasks do not block
    work on this task, and errors that already existed before this operation
    (baseline) are not blamed on it. This is the pattern extend_budget used;
    it is now the single rule for every mutation.
    """
    task_ids = set(task_ids)
    baseline = set(baseline)
    return [message for message in errors if message not in baseline
            and (error_owner(message) is None or not task_ids or error_owner(message) in task_ids)]


def board_bytes(tasks):
    groups = {"BACKLOG": {"draft", "ready", "blocked"}, "IN_PROGRESS": {"running", "verifying", "review", "verified"}, "DONE": {"done", "cancelled"}}
    output = {}
    for name, states in groups.items():
        lines = [BOARD_MARKER, f"# {name}", "", "| Task | Title | Status |", "| --- | --- | --- |"]
        for ident, task in sorted(tasks.items()):
            if task.get("status") in states:
                title = str(task.get("title") or "Pending definition").replace("|", "\\|").replace("\n", " ")
                lines.append(f"| [{ident}](items/{ident}.json) | {title} | {task['status']} |")
        lines += ["", "verified 表示验证/审查完成、等待所属交付验收；不等于已经合并。", ""]
        output[f"tasks/{name}.md"] = "\n".join(lines).encode("utf-8")
    return output


def project_state_bytes(project, tasks, helper="scripts/project_workflow.py", brief=None, policy=None,
                        research_done=False, ui_accepted=False):
    """The same human-readable facts used in chat, never a second state store."""
    from workflow_progress import build
    progress = build(project, policy or {}, brief or {}, tasks, research_done=research_done, ui_accepted=ui_accepted)
    text = (BOARD_MARKER + "\n# 项目状态\n\n" + progress["markdown"]
            + "\n运行 `python " + helper + " progress --root .` 生成对话用进度；start 给出实际下一步。\n"
            + "本文件由需求、PROJECT、任务和检查点生成；实际证据与进程仍须核对。\n")
    return text.encode("utf-8")


def resume_bytes(project, tasks, journal_text, helper="scripts/project_workflow.py", brief=None, policy=None,
                 research_done=False, ui_accepted=False, action=None, notes_prefix="../"):
    """DeepSeek-style handoff note: current facts, open items, recent events, how to continue."""
    from workflow_progress import build
    progress = build(project, policy or {}, brief or {}, tasks, action=action, card_prefix=notes_prefix + "tasks/cards/",
                     research_done=research_done, ui_accepted=ui_accepted)
    lines = [journal_line for journal_line in journal_text.splitlines() if journal_line.startswith("- ")]
    recent = lines[-12:]
    open_notes = [line for line in lines if " · note/todo · " in line or " · note/decision · " in line or " · note/context · " in line][-12:]
    lessons = [line for line in lines if " · note/lesson · " in line][-8:]
    text = [BOARD_MARKER, "# 接手与恢复笔记", "",
            "任何 Agent 接手前先读本文件，再运行 `python " + helper + " resume --root .`。本文件由任务记录、检查点和日志生成；事实以 JSON 记录和原始证据为准。", "",
            "## 当前状态", "", progress["markdown"].rstrip(), "",
            "## 未完成的上下文、决策与待办（Agent 笔记）", ""]
    text += open_notes or ["- 暂无记录；用 `note --kind context|decision|todo --text ...` 保存需要延续的判断。"]
    text += ["", "## 教训", ""] + (lessons or ["- 暂无记录。"])
    text += ["", "## 最近事件", ""] + (recent or ["- 暂无事件。"])
    text += ["", "## 如何继续", "",
             "1. 运行 resume；有 controller.lock 或 running 的 RUN 先核对进程，再决定 recover。",
             "2. 阻塞任务先读任务卡的最近检查点和原始日志；scope/protocol/action_required/evidence 类阻塞用 `unblock --task --source --note` 带说明解锁，不新建任务。",
             "3. 已确认但尚未拆分的需求见上表；只有全部需求关联到已验收任务并获用户确认才 `accept --project-complete`。",
             "4. 完整日志：[JOURNAL.md](JOURNAL.md)；任务总览：[PROJECT_STATE.md](" + notes_prefix + "tasks/PROJECT_STATE.md)。", ""]
    return "\n".join(text).encode("utf-8")


def initialization_plan(root, kind, name, allow_existing=False, full_docs=False):
    root = Path(root).resolve()
    package = Path(__file__).resolve().parent.parent
    assets = package / "assets" / "project"
    if not assets.is_dir():
        raise ValueError("Run init from the full startup package; the copied project helper supports execution and recovery, not creating another project.")
    if not name or any(char in name for char in "\r\n"):
        raise ValueError("Project name must be a nonempty single line")
    plan = {}
    for source in sorted(assets.rglob("*")):
        if source.is_file():
            rel = source.relative_to(assets).as_posix()
            optional = rel.startswith(("memory/", "lessons/")) or (
                rel.startswith("docs/") and rel not in {
                    "docs/README.md.template", "docs/PRODUCT.md.template", "docs/HANDOFF.md.template",
                    "docs/ADR/README.md.template", "docs/ADR/DECISION-TEMPLATE.md.template"})
            if optional and not full_docs and not (kind == "refactor" and rel == "docs/BASELINE.md.template"):
                continue
            if rel.endswith(".template"):
                rel = rel[:-9]
                plan[rel] = source.read_text(encoding="utf-8").replace("{{PROJECT_NAME}}", name).encode("utf-8")
            else:
                plan[rel] = source.read_bytes()
    project = json.loads(plan["tasks/PROJECT.json"])
    project.update(name=name, kind=kind, created_at_utc=iso(utc_now()), workflow_kit="workflow-kit")
    plan["tasks/PROJECT.json"] = encoded(project)
    plan["tasks/PROJECT_STATE.md"] = project_state_bytes(project, {})
    for source in sorted((package / "references").rglob("*")):
        if source.is_file():
            plan["docs/workflow/" + source.relative_to(package / "references").as_posix()] = source.read_bytes()
    note = package / ".agents/notes/implemented/process/2026-09-13-portable-project-workflow.md"
    plan["docs/workflow/design-note.md"] = note.read_bytes()
    plan["scripts/project_workflow.py"] = Path(__file__).read_bytes()
    runtime = Path(__file__).with_name("workflow_runtime.py")
    if runtime.is_file():
        plan["scripts/workflow_runtime.py"] = runtime.read_bytes()
    from workflow_bootstrap import classic_binding
    plan["scripts/workflow_bootstrap.py"] = Path(__file__).with_name("workflow_bootstrap.py").read_bytes()
    plan["scripts/workflow_intake.py"] = Path(__file__).with_name("workflow_intake.py").read_bytes()
    plan["scripts/workflow_progress.py"] = Path(__file__).with_name("workflow_progress.py").read_bytes()
    plan["notes/JOURNAL.md"] = (BOARD_MARKER.replace("generated view; edit task JSON instead", "append-only journal; use checkpoint/note")
                                + "\n# 项目日志\n\n工具在每个关键事件后追加一行；Agent 用 note 追加上下文、决策、待办和教训。不要手工改写历史行。\n\n").encode("utf-8")
    plan["notes/RESUME.md"] = resume_bytes(project, {}, plan["notes/JOURNAL.md"].decode("utf-8"))
    plan["tasks/.gitattributes"] = b"# workflow-kit records and evidence are hashed byte-for-byte; never convert line endings.\n* -text\n"
    plan.update(board_bytes({}))
    plan[CLASSIC_BINDING] = encoded(classic_binding(plan))
    conflicts = [rel for rel in plan if inside(root, rel).exists()]
    if conflicts and not allow_existing:
        raise ValueError("Refusing all writes; existing target files: " + ", ".join(sorted(conflicts)))
    return plan


def initialize(root, kind, name, write=False, full_docs=False):
    root = Path(root).resolve()
    if inside(root, ISOLATED_BINDING).exists() or inside(root, CLASSIC_BINDING).exists():
        raise ValueError("Refusing all writes; an existing workflow binding must be resumed or repaired, never initialized again")
    plan = initialization_plan(root, kind, name, full_docs=full_docs)
    if not write:
        return {"written": False, "root": str(root), "files": sorted(plan)}
    if not root.parent.is_dir():
        raise ValueError("Target parent must already exist")
    created_files, created_dirs = [], []
    try:
        if not root.exists():
            root.mkdir()
            created_dirs.append(root)
        for rel, data in plan.items():
            target = inside(root, rel)
            missing, parent = [], target.parent
            while not parent.exists():
                missing.append(parent)
                parent = parent.parent
            for directory in reversed(missing):
                directory.mkdir()
                created_dirs.append(directory)
            with target.open("xb") as stream:
                created_files.append(target)
                stream.write(data)
    except (OSError, ValueError):
        for path in reversed(created_files):
            if path.resolve().is_relative_to(root):
                path.unlink()
        for path in reversed(created_dirs):
            if path.resolve() == root or path.resolve().is_relative_to(root):
                try:
                    path.rmdir()
                except OSError:
                    pass
        raise
    return {"written": True, "root": str(root), "files": sorted(plan), "application_work_authorized": False}


def main(argv=None):
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8")
    argv = list(sys.argv[1:] if argv is None else argv)
    runtime_commands = {"bootstrap", "doctor", "start", "progress", "review-packet", "adopt", "intake", "onboard", "research", "cards", "checkpoint", "prepare", "begin", "finish", "verify", "review", "feedback", "dispatch", "review-cli", "run", "next", "recover", "extend", "batch", "accept",
                        "unblock", "cancel", "diff", "note", "recompute", "rebind", "resume"}
    if argv and argv[0] in runtime_commands:
        from workflow_runtime import main as runtime_main
        return runtime_main(argv)
    parser = argparse.ArgumentParser(description=__doc__, epilog="Workflow commands (each accepts --help): " + ", ".join(sorted(runtime_commands)))
    sub = parser.add_subparsers(dest="command", required=True)
    init = sub.add_parser("init", help="Preview or create governance files; never overwrite")
    init.add_argument("--root", required=True)
    init.add_argument("--kind", choices=("new", "refactor"), required=True)
    init.add_argument("--name", required=True)
    init.add_argument("--write", action="store_true")
    init.add_argument("--full-docs", action="store_true", help="Also create optional document/memory templates; minimal documents are the default")
    for command in ("check", "boards"):
        sub.add_parser(command).add_argument("--root", default=".")
    snap = sub.add_parser("snapshot", help="Hash explicit source/test/config paths")
    snap.add_argument("--root", default=".")
    snap.add_argument("--paths", nargs="+", required=True)
    snap.add_argument("--output", required=True)
    snap.add_argument("--allow-missing", action="store_true", help="Record explicitly absent inputs for a new project")
    sub.add_parser("stamp", help="Print current UTC time and a unique run id")
    definition = sub.add_parser("definition", help="Compute a task controlled-definition digest without editing it")
    definition.add_argument("--task", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "init":
            result = initialize(args.root, args.kind, args.name, args.write, args.full_docs)
        elif args.command == "stamp":
            result = {"utc": iso(utc_now()), "run_id": "RUN-" + uuid.uuid4().hex}
        elif args.command == "definition":
            result = {"definition_digest": task_definition(read_json(args.task))}
        elif args.command == "snapshot":
            root = Path(args.root).resolve()
            output = inside(root, args.output)
            if any(output == inside(root, name) or output.is_relative_to(inside(root, name)) for name in args.paths):
                raise ValueError("Snapshot output must be outside all selected input paths")
            result = capture(root, args.paths, allow_missing=args.allow_missing)
            if output.exists():
                raise ValueError("Snapshot output exists; choose a new evidence name")
            output.parent.mkdir(parents=True, exist_ok=True)
            inside(root, args.output)
            write_exclusive(output, encoded(result))
            result = {"manifest": args.output, "digest": result["digest"], "files": len(result["files"])}
        else:
            root = Path(args.root).resolve()
            result = check_project(root)
            if args.command == "boards" and result["ok"]:
                errors = []
                task_items = records(root, "tasks/items", "TASK-", errors)
                plans = board_bytes(task_items)
                for rel in plans:
                    path = workflow_inside(root, rel)
                    if path.exists() and not path.read_text(encoding="utf-8-sig").startswith(BOARD_MARKER):
                        raise ValueError(f"Refusing to overwrite manually maintained board: {rel}")
                for rel, data in plans.items():
                    path = workflow_inside(root, rel)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
                result["boards_written"] = sorted(plans)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0 if result.get("ok", True) else 1
    except (ValueError, OSError, KeyError, TypeError, AttributeError) as error:
        print(json.dumps({"ok": False, "errors": [str(error)]}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
