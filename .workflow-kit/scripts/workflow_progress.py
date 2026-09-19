"""Project facts rendered for chat/native plans and the existing Markdown view."""
# Note: visible progress without duplicate state — ../.agents/notes/implemented/process/
# 2026-09-13-portable-project-workflow.md; installed copy: ../docs/workflow/design-note.md.
from collections import Counter


STATUSES = {"draft": "待准备", "ready": "待执行", "running": "进行中", "verifying": "验证中",
            "review": "审查中", "verified": "已验证，待验收", "done": "已验收", "blocked": "阻塞", "cancelled": "已取消"}
ACTION_LABELS = {"ask": "补充尚未确认的信息", "onboard": "确认执行摘要", "research": "查验参考与技术限制",
    "assess_existing": "结合意向分析现有项目", "prepare": "准备下一项已确认的工作", "begin": "开始当前任务", "run": "推进当前任务",
    "verify": "运行当前候选的检查", "review": "审查实际差异与证据", "repair": "在原任务内处理已定位问题",
    "prepare_ui_preview": "制作可点击界面预览", "accept_ui_preview": "展示预览并确认视觉与交互",
    "accept_or_prepare_next_authorized_task": "展示成果，验收或继续已确认范围", "prepare_or_finish_project": "核对完整需求后继续或交付",
    "assessment_complete": "展示评估结论与后续选项", "complete": "项目已验收", "paused": "保留现场，等待继续",
    "replan": "依据失败证据调整方法", "budget_decision": "保留现场并处理额度安排",
    "inspect_active_run": "核对现有进程和日志", "inspect_controller": "核对现有控制器", "recover_lock": "核对中断现场并恢复",
    "inspect_blocker": "处理当前阻塞", "repair_records_or_refresh_stale_evidence": "核对状态或失效证据"}


def clean(value):
    return str(value or "待确认").replace("|", "\\|").replace("\n", " ").replace("\r", " ")


def build(project, policy, brief, tasks, action=None, card_prefix="cards/", research_done=False, ui_accepted=False):
    action = action or {}
    next_step = action.get("next")
    approved = policy.get("approval", {}).get("status") == "approved"
    completed = project.get("stage") == "complete"
    assessment_only = next_step == "assessment_complete" or (
        not policy.get("authority", {}).get("code") and brief.get("refactor", {}).get("decision") in {"keep", "assess_only"})
    ui_mode = policy.get("ui", {}).get("mode", "none") if approved else brief.get("ui", {}).get("mode", "pending")
    requirements = [item for item in brief.get("requirements", []) if isinstance(item, dict)]
    included = [item for item in requirements if item.get("in_scope")]
    covered = {ref for task in tasks.values() if task.get("status") != "cancelled" for ref in task.get("requirement_refs", [])}
    unplanned = [item for item in included if item.get("id") not in covered]
    counts = dict(Counter(task.get("status") for task in tasks.values()))
    ordered = sorted(tasks.values(), key=lambda task: (
        {"running": 0, "verifying": 1, "review": 2, "blocked": 3, "ready": 4, "verified": 5, "draft": 6, "done": 7, "cancelled": 8}.get(task.get("status"), 9), task.get("id", "")))
    active = next((task for task in ordered if task.get("status") in {"running", "verifying", "review", "blocked", "ready"}), None)
    if not approved:
        current = "intake"
    elif not research_done and not assessment_only:
        current = "discovery"
    elif ui_mode == "preview_first" and not ui_accepted and not assessment_only:
        current = "preview"
    elif assessment_only:
        current = "discovery" if active else "delivery"
    elif active:
        current = "validation" if active.get("status") in {"verifying", "review"} else "implementation"
    elif counts.get("verified") or completed or assessment_only:
        current = "delivery"
    else:
        current = "implementation"
    quality = brief.get("quality") or {}
    implementation_goal = "落实已确认的完整范围，逐步交付并保持已验收行为"
    if included:
        implementation_goal += "：" + "；".join(item.get("description", "") for item in included[:3])
    phases = [
        {"id": "intake", "title": "需求与目标", "goal": "明确目标、已有 Bug、新功能、其他要求、质量目标和执行边界"},
        {"id": "discovery", "title": "分析与方案", "goal": "记录参考、原始基线状态与限制，说明维护、稳定和性能取舍，并确认路线"},
        {"id": "preview", "title": "界面预览", "goal": "验证关键流程、整体设计和控件完整状态，确认后沿用前端实现"},
        {"id": "implementation", "title": "分步实施", "goal": implementation_goal},
        {"id": "validation", "title": "回归与审查", "goal": "以需求、失败路径、适用界面检查、维护性和性能证据核对当前组合候选"},
        {"id": "delivery", "title": "验收与交付", "goal": "核对完整范围，交付可运行成果、使用说明及适用的恢复办法"},
    ]
    for phase in phases:
        ident = phase["id"]
        skipped = (ident == "preview" and ui_mode in {"none", "existing"}) or (assessment_only and ident in {"preview", "implementation", "validation"})
        complete = completed or (ident == "intake" and approved) or (ident == "discovery" and research_done) or (ident == "preview" and ui_accepted)
        phase["status"] = "不适用" if skipped else "已完成" if complete else "当前" if ident == current else "待推进"
        if not completed and ident in {"implementation", "validation"} and ident != current and tasks and not skipped:
            phase["status"] = "分批推进"
    items = []
    for task in ordered[:12]:
        points = task.get("checkpoints", [])
        items.append({"id": task["id"], "title": task.get("title"), "status": task.get("status"),
            "status_label": STATUSES.get(task.get("status"), "待核对"), "goal": task.get("objective"),
            "last_checkpoint": points[-1].get("note") if points else None,
            "next_action": points[-1].get("next_action") if points else None,
            "card": card_prefix + task["id"] + ".md"})
    value = {"project": project.get("name"), "goal": brief.get("goal") or "等待用户确认目标",
        "stage": next(phase["title"] for phase in phases if phase["id"] == current),
        "stage_goal": next(phase["goal"] for phase in phases if phase["id"] == current),
        "phases": phases, "tasks": items, "task_counts": counts, "known_tasks": len(tasks),
        "hidden_tasks": max(0, len(tasks) - len(items)), "unplanned_requirements": unplanned,
        "deferred_requirements": [item for item in requirements if not item.get("in_scope")],
        "acceptance": brief.get("acceptance", []), "quality": quality,
        "blockers": [str(reason) for task in ordered for reason in task.get("blockers", [])],
        "next_action": ACTION_LABELS.get(next_step, "结合当前任务、验收与实际文件确定下一步"),
        "actual_next": next_step, "project_complete": completed}
    value["intake_confirmed"] = sum(state in {"confirmed", "not_applicable"} for state in action.get("topics", {}).values())
    value["intake_pending"] = list(action.get("missing", []))
    value["native_plan"] = [{"step": phase["title"] + "：" + phase["goal"],
        "status": "completed" if phase["status"] in {"已完成", "不适用"} else "in_progress" if phase["status"] == "当前" else "pending"}
        for phase in phases if phase["status"] != "不适用"]
    # Host-neutral task list: the shape most todo/plan/task panels accept.
    generic = {"running": "in_progress", "verifying": "in_progress", "review": "in_progress", "blocked": "blocked",
               "ready": "pending", "draft": "pending", "verified": "completed", "done": "completed", "cancelled": "completed"}
    value["native_tasks"] = [{"id": item["id"], "title": str(item["title"] or item["id"]),
        "status": generic.get(item["status"], "pending"), "detail": item["status_label"],
        "next": str(item.get("next_action") or "")} for item in items]
    value["needs_user"] = user_decisions(value, action)
    value["compact"] = render_compact(value)
    value["markdown"] = render(value)
    value["display_instruction"] = ("展示分三档，按宿主实际能力选最高一档：1) 有原生任务/计划面板：用 native_tasks 同步任务、native_plan 同步阶段，对话里只说变化；"
        "2) 没有面板：日常轮次贴 compact，首次、阶段切换、验收和阻塞时贴 markdown；3) 只能输出文本：同 2。"
        "needs_user 非空时必须单独列出等用户决定的事项。文件已生成不等于用户已看到；不要推算全项目百分比。")
    return value


def user_decisions(value, action):
    """Everything waiting on the owner, so the user never has to search the transcript."""
    items = ["回答：" + clean(question.get("question")) for question in action.get("questions", []) or []]
    hints = {"accept_ui_preview": "查看可点击预览并确认视觉与交互（accept 或 feedback）",
             "accept_or_prepare_next_authorized_task": "验收已验证的成果，或确认继续下一项已确认范围",
             "budget_decision": "决定是否为当前任务追加时间/修复额度（extend）",
             "unblock": "确认阻塞处置：撤销越界改动或认可归属后解锁（unblock）",
             "prepare_or_finish_project": "核对未拆分需求；全部完成才确认项目交付",
             "replan": "阅读失败证据后同意换一种实现方法",
             "assessment_complete": "阅读评估结论，选择保持现状、局部修补、渐进重构或迁移"}
    if action.get("next") in hints:
        items.append(hints[action["next"]])
    return items


def render_compact(value):
    """About ten lines: enough for a routine turn without re-pasting the whole board."""
    counts = value["task_counts"]
    lines = ["**" + clean(value["project"]) + " · " + clean(value["stage"]) + "**：" + clean(value["stage_goal"])]
    active = [item for item in value["tasks"] if item["status"] in {"running", "verifying", "review", "blocked", "ready"}][:3]
    for item in active:
        lines.append("- " + clean(item["id"] + " " + str(item["title"])) + "：" + item["status_label"]
                     + ("，下一步 " + clean(item["next_action"]) if item.get("next_action") else ""))
    if not active and not value["tasks"]:
        lines.append("- 尚未建立实施任务（不等于完成）")
    lines.append(f"- 任务 {value['known_tasks']}：已验收 {counts.get('done', 0)}，待验收 {counts.get('verified', 0)}，阻塞 {counts.get('blocked', 0)}"
                 + (f"；未拆分需求 {len(value['unplanned_requirements'])}" if value["unplanned_requirements"] else ""))
    if value["blockers"]:
        lines.append("- 阻塞：" + clean(value["blockers"][0]) + (f"（另 {len(value['blockers']) - 1} 项）" if len(value["blockers"]) > 1 else ""))
    lines.append("- 下一步：" + value["next_action"])
    if value["needs_user"]:
        lines.append("- **需要你决定**：" + "；".join(value["needs_user"][:3]))
    return "\n".join(lines) + "\n"


def render(value):
    lines = ["**项目进度 · " + clean(value["project"]) + "**", "", "目标：" + clean(value["goal"]),
        "", "当前阶段：**" + clean(value["stage"]) + "**", "", "阶段目标：" + clean(value["stage_goal"]),
        "", "| 阶段 | 目标 | 状态 |", "| --- | --- | --- |"]
    for phase in value["phases"]:
        lines.append("| " + " | ".join(clean(phase[key]) for key in ("title", "goal", "status")) + " |")
    if value["intake_pending"]:
        lines += ["", f"问答：已确认 {value['intake_confirmed']} 个主题，仍有 {len(value['intake_pending'])} 个主题待补充或核对；按下一轮问题继续，已有回答不重问。"]
    if value["acceptance"]:
        lines += ["", "**完整验收目标**：" + "；".join(clean(item) for item in value["acceptance"])]
    quality = value.get("quality") or {}
    if quality:
        performance = quality.get("performance") or {}
        lines += ["", "**质量目标**：维护性—" + clean(quality.get("maintainability")) + "；稳定性—" + clean(quality.get("stability")),
                  "", "**性能安排**：" + ("；".join(clean(item) for item in performance.get("targets", []))
                       if performance.get("mode") == "measure" else clean(performance.get("rationale")))]
    counts = value["task_counts"]
    lines += ["", f"已建任务 {value['known_tasks']} 项：已验收 {counts.get('done', 0)}，待验收 {counts.get('verified', 0)}，阻塞 {counts.get('blocked', 0)}。"]
    if value["tasks"]:
        lines += ["", "| 任务 | 状态 | 目标 / 下一步 |", "| --- | --- | --- |"]
        for item in value["tasks"]:
            label = clean(item["id"] + " · " + str(item["title"]))
            lines.append("| [" + label + "](<" + item["card"] + ">) | " + item["status_label"] + " | "
                         + clean((str(item["last_checkpoint"]) + "；" if item.get("last_checkpoint") else "")
                                 + str(item.get("next_action") or item["goal"])) + " |")
    else:
        lines += ["", "当前尚未建立实施任务；这不表示项目已完成。"]
    if value["hidden_tasks"]:
        lines += ["", f"另有 {value['hidden_tasks']} 项记录可在任务总览查看。"]
    if value["unplanned_requirements"]:
        lines += ["", "**已确认但尚未拆分的需求**：" + "；".join(clean(item["description"]) for item in value["unplanned_requirements"])]
    if value["deferred_requirements"]:
        lines += ["", "**本轮暂缓**：" + "；".join(clean(item["description"]) for item in value["deferred_requirements"])]
    lines += ["", "**阻塞**：" + ("；".join(clean(item) for item in value["blockers"][:3]) or "无已记录阻塞"),
              "", "**下一步**：" + value["next_action"]]
    if value.get("needs_user"):
        lines += ["", "**需要你决定**：" + "；".join(value["needs_user"])]
    lines += ["", "任务数量只描述已建立的工作；完整目标、尚未拆分需求和最终验收仍须核对。"]
    return "\n".join(lines) + "\n"
