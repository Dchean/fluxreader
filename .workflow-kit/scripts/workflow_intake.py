"""Small, resumable intake audit; user answers are distinct from agent proposals."""
# Note: confirmed choices and portable records — ../.agents/notes/implemented/process/
# 2026-09-13-portable-project-workflow.md; installed copy: ../docs/workflow/design-note.md.
from __future__ import annotations

import copy
import hashlib
import json


HOSTS = {"current_agent", "codex-cli", "claude-code-cli", "custom"}
INTENTS = {"assess", "targeted", "refactor"}
DIRECTIONS = {"assess_only", "keep", "targeted", "incremental", "replace"}
QUESTIONS = {
    "refactor_intent": ("对这个已有项目，你希望先评估，解决具体问题，还是已经决定重构？",
        ["先评估，再由我选择", "先解决具体问题", "已决定重构，确认范围后实施"],
        "可以补充目前最困扰你的地方；不知道技术原因也没关系。先确认意向，再做针对性分析。"),
    "goal": ("这次最想解决什么问题，主要给谁使用？", [],
        "补充一条最重要的使用流程即可；已有项目说明当前痛点或近期计划，技术原因由 Agent 检查。"),
    "scope": ("这轮做到什么就算完成，哪些内容先不做？", [],
        "Agent 先列完整目标和阶段交付，再分步验证；不会因为省工作量缩减已确认需求。可以调整优先级或补充限制。"),
    "requirements": ("除了已说明的目标，还有已知 Bug、想增加的功能或其他要求吗？",
        ["补充 Bug、功能或其他需求", "暂时没有补充", "先由 Agent 排查并给出建议"],
        "Bug 可补充触发场景、现在的表现和期望结果；功能可说明用途与优先级。Agent 分开记录已确认需求和待确认发现，不让重构吞掉功能诉求。"),
    "quality": ("常用设备、数据规模，以及不能接受的故障或卡顿有哪些？",
        ["由 Agent 先检查并提出可验证目标", "补充已有质量或性能要求"],
        "默认以可维护、稳定和性能为目标，不以实现工作量少作取舍；复杂度须有实际收益。性能结合真实负载测量，冲突时给证据和建议。"),
    "compatibility": ("改动时有哪些功能、接口、数据或界面必须保留？",
        ["保留现有行为，只改善指定问题", "允许调整列明的部分"],
        "Agent 应结合检查结果列出受影响项；数据迁移、技术栈替换和 UI 改版需要具体说明。"),
    "refactor_decision": ("结合刚才的检查，你希望采用哪条改进路线？",
        ["保持现状或结束评估", "局部修补", "渐进重构", "替换或迁移"],
        "先展示实际证据、建议范围、收益、代价和验证方法，再由你选择；建议不会自动成为决定。"),
    "execution": ("编程由当前 Agent 全部处理，还是让它调用 CLI 协作？",
        ["当前 Agent 全部处理（推荐起步）", "Agent + CLI"],
        "CLI 可选。选择协作后再确认具体 CLI 和承担的工作；已安装某个 CLI 或当前宿主名称都不代表你选择了它。"),
    "review": ("完成后采用哪种检查安排？",
        ["真实测试 + 独立上下文审查（具备能力时推荐）", "真实测试 + 有证据的自审"],
        "Agent 包办也可使用未参与实现的新 Agent 上下文，同一模型即可，不要求 CLI。没有独立上下文时如实说明；高风险任务不能用自审代替。"),
    "ui": ("这次界面希望怎么处理？",
        ["先看可点击前端预览", "保留现有设计，检查受影响界面", "没有界面"],
        "有界面时补充风格参考和目标设备。新界面或明显改版推荐先预览，确认控件展开、弹窗和交互后再落地。"),
    "delivery": ("完成后你希望在哪里、以什么方式使用它？", [],
        "例如本机运行、打包安装或部署网站；说明是否需要真实外部服务。交付到服务器不自动授权发布或操作生产数据。"),
    "services": ("需要连接的真实服务、账号和数据，允许操作到什么范围？",
        ["先用测试环境或数据副本", "使用列明的真实服务与操作范围"],
        "Agent 应列出具体服务、读写边界和可能费用；不要在回答、文档或任务里粘贴密钥。没有外部服务就跳过此问。"),
    "budget": ("可以按这份时间、修复次数和费用安排推进吗？",
        ["接受展示的预算", "调整预算或费用上限"],
        "先展示每批推进方式与防空转上限。单任务预算用于控制失控，不能据此降低项目质量或删减范围；较长工作分段完成，追加额度沿用明确授权。"),
    "authority": ("这些工作可以自动完成，哪些动作要先交给你确认？",
        ["按展示的范围自动推进", "每批完成后确认", "先只评估"],
        "展示代码、依赖、CI、提交、推送和发布的具体边界；沿用已有授权，只补缺项，不把接入工作流当成业务授权。"),
}


def text(value):
    return isinstance(value, str) and bool(value.strip())


def texts(value, nonempty=True):
    return isinstance(value, list) and (bool(value) or not nonempty) and all(text(item) for item in value)


def fingerprint(value):
    data = json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(data.encode("utf-8")).hexdigest()


def merge(base, patch):
    if not isinstance(patch, dict):
        raise ValueError("Intake answers must be an object")
    result = copy.deepcopy(base)
    for key, value in patch.items():
        if key == "confirmations" and isinstance(value, dict):
            # A new answer replaces the entire old receipt, including its digest.
            result.setdefault(key, {}).update(copy.deepcopy(value))
        elif isinstance(value, dict) and isinstance(result.get(key), dict):
            result[key] = merge(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result


def topic_values(brief, kind):
    execution = brief.get("execution") or {}
    refactor = brief.get("refactor") or {}
    delivery = brief.get("delivery") or {}
    values = {}
    if kind == "refactor":
        values["refactor_intent"] = refactor.get("intent")
    values["goal"] = {key: brief.get(key) for key in ("goal", "audience")}
    values["scope"] = {key: brief.get(key) for key in ("acceptance", "non_goals")}
    values["requirements"] = brief.get("requirements")
    values["quality"] = brief.get("quality")
    if kind == "refactor":
        values["compatibility"] = brief.get("compatibility")
        values["refactor_decision"] = {key: refactor.get(key) for key in ("decision", "assessment")}
    values["execution"] = {key: execution.get(key) for key in ("mode", "coder", "worker_argv", "model", "allow_non_git")}
    values["review"] = {key: execution.get(key) for key in ("reviewer", "review_mode", "reviewer_argv")}
    values["ui"] = brief.get("ui")
    values["delivery"] = {key: delivery.get(key) for key in ("target", "external_services")}
    if delivery.get("external_services") in {"sandbox", "production"}:
        values["services"] = {key: delivery.get(key) for key in ("external_services", "data_boundary")}
    values["budget"] = {key: brief.get(key) for key in ("budget", "financial", "recovery")}
    values["authority"] = brief.get("authority")
    return values


def resolve(defaults, previous, submitted, kind):
    brief = merge(merge(defaults, previous), submitted)
    for key in ("execution", "refactor", "delivery", "ui", "budget", "financial", "recovery", "authority", "confirmations", "quality"):
        if not isinstance(brief.get(key), dict):
            raise ValueError("Intake " + key + " must be an object")
    values = topic_values(brief, kind)
    for topic, receipt in brief["confirmations"].items():
        if topic in values and isinstance(receipt, dict) and "value_digest" not in receipt:
            if receipt.get("status") in {"confirmed", "not_applicable"}:
                receipt["value_digest"] = fingerprint(values[topic])
    return brief


def question(topic, brief=None):
    title, options, supplement = QUESTIONS[topic]
    item = {"id": topic, "question": title, "options": options, "supplement": supplement,
            "allow_supplement": True}
    if topic == "refactor_decision" and brief:
        item["assessment"] = brief.get("refactor", {}).get("assessment")
    if brief and topic in {"budget", "authority"}:
        item["proposed_values"] = topic_values(brief, "new")[topic]
    return item


def initial_questions(kind):
    topics = ("refactor_intent", "goal", "requirements") if kind == "refactor" else ("goal", "requirements", "scope")
    return [question(topic) for topic in topics]


def audit(brief, kind):
    """Check completeness and stale confirmations, not the authenticity of a chat quote."""
    values = topic_values(brief, kind)
    execution = brief.get("execution") or {}
    refactor = brief.get("refactor") or {}
    ui = brief.get("ui") or {}
    delivery = brief.get("delivery") or {}
    finance = brief.get("financial") or {}
    authority = brief.get("authority") or {}
    confirmations = brief.get("confirmations") or {}
    requirements = brief.get("requirements")
    quality = brief.get("quality") or {}
    performance = quality.get("performance") or {}
    valid_requirements = (isinstance(requirements, list) and all(isinstance(item, dict)
        and text(item.get("id")) and item.get("kind") in {"bug", "feature", "improvement", "constraint"}
        and text(item.get("description")) and type(item.get("in_scope")) is bool
        and texts(item.get("acceptance"), nonempty=item.get("in_scope", False)) for item in requirements))
    if valid_requirements:
        valid_requirements = len({item["id"] for item in requirements}) == len(requirements)
    validity = {
        "goal": text(brief.get("goal")) and text(brief.get("audience")),
        "scope": texts(brief.get("acceptance")) and texts(brief.get("non_goals"), nonempty=False),
        "requirements": valid_requirements,
        "quality": (quality.get("priority") == "quality_first" and text(quality.get("maintainability"))
            and text(quality.get("stability")) and isinstance(performance, dict)
            and ((performance.get("mode") == "measure" and text(performance.get("workload")) and texts(performance.get("targets")))
                 or (performance.get("mode") == "not_applicable" and text(performance.get("rationale"))))),
        "execution": (execution.get("mode") in {"agent_only", "agent_cli"}
            and execution.get("coder") in HOSTS and execution.get("reviewer") in HOSTS
            and (execution.get("mode") == "agent_only") == (execution.get("coder") == execution.get("reviewer") == "current_agent")),
        "review": execution.get("review_mode") in {"self_review_allowed", "independent_required"} and execution.get("reviewer") in HOSTS,
        "ui": ui.get("mode") == "none" or (ui.get("mode") in {"existing", "preview_first"}
            and text(ui.get("style_intent")) and texts(ui.get("target_devices"))),
        "delivery": text(delivery.get("target")) and delivery.get("external_services") in {"none", "sandbox", "production"},
        "services": text(delivery.get("data_boundary")),
        "budget": finance.get("mode") in {"none", "limit"} and isinstance(brief.get("budget"), dict),
        "authority": (all(type(authority.get(key)) is bool for key in ("documentation", "baseline", "code", "dependencies", "ci"))
            and all(authority.get(key) in {"ask", "allowed", "forbidden"} for key in ("commit", "push", "pull_request", "merge", "release"))),
    }
    assessment = refactor.get("assessment") or {}
    assessment_ready = (isinstance(assessment, dict) and texts(assessment.get("evidence"))
        and assessment.get("recommendation") in DIRECTIONS and text(assessment.get("tradeoffs")))
    if kind == "refactor":
        validity.update(refactor_intent=refactor.get("intent") in INTENTS,
            compatibility=text(brief.get("compatibility")) or texts(brief.get("compatibility")),
            refactor_decision=assessment_ready and refactor.get("decision") in DIRECTIONS
                and (not authority.get("code") or refactor.get("decision") in {"targeted", "incremental", "replace"}))
    missing, statuses = [], {}
    for topic, value in values.items():
        receipt = confirmations.get(topic)
        valid_receipt = isinstance(receipt, dict) and receipt.get("value_digest") == fingerprint(value)
        confirmed = (valid_receipt and receipt.get("status") == "confirmed"
                     and text(receipt.get("source")) and text(receipt.get("answer")))
        inapplicable = (topic == "ui" and ui.get("mode") == "none" and valid_receipt
                       and receipt.get("status") == "not_applicable" and text(receipt.get("reason")))
        statuses[topic] = "confirmed" if confirmed else "not_applicable" if inapplicable else "unconfirmed"
        if not validity[topic] or not (confirmed or inapplicable):
            missing.append(topic)
            if not validity[topic]:
                statuses[topic] = "incomplete_or_inconsistent"
    next_step = "onboard" if not missing else "ask"
    ask_topics = missing[:3]
    if kind == "refactor":
        if "refactor_intent" in missing:
            ask_topics = [key for key in ("refactor_intent", "goal", "requirements") if key in missing]
        elif not assessment_ready:
            ask_topics = [key for key in ("goal", "requirements", "compatibility") if key in missing]
            if not ask_topics:
                next_step = "assess_existing"
    return {"ok": True, "stage": "intake", "next": next_step, "ready_for_onboard": not missing,
            "missing": missing, "topics": statuses, "questions": [question(key, brief) for key in ask_topics],
            "instruction": "按提问＋补充逐轮补缺项；引用真实回答，建议保持 proposed。先确认已有项目意向，再分析并让用户选路线。答案保存后沿用；未答、超时或默认值不算同意。"}
