# -*- coding: utf-8 -*-
"""Patch the flake observation in the TASK-080 r2 report with the stronger evidence. Deleted after use."""
import io, json

p = r"D:\fluxreader\.workflow-kit\tasks\evidence\TASK-080-review-report-r2.json"
d = json.load(io.open(p, encoding="utf-8"))

old_obs = (
    "(1) TASK-080-cumulative-diff.txt 对已披露 flake 的机理说明与实测不符——我独立复现了该 flake，"
    "并在隔离运行中发现**两种**断言都曾失败（on 例 left:0/right:1，off 例 left:3/right:2），"
    "而两只用例各自单独跑 18/18 全通过，指向同进程内两只用例互相干扰，而非作者所述的"
    "「并发 smart-dedup 路径的读-写窗口，计数可为 0 或 2」；该机理订正应留给已记录的独立跟进项。"
)
new_obs = (
    "(1) TASK-080-cumulative-diff.txt 对已披露 flake 的机理说明**与代码和实测均不符**，但该文件是审查上下文附件、"
    "不在候选清单内，且 flake 本身确与本任务无关，故按指示仅记为观察。我的独立复现与判定："
    "该 flake 真实存在（完整测试二进制 12 次隔离运行中 2 次失败：on 例 left:0/right:1、off 例 left:3/right:2），"
    "而两只用例用 `--exact` 各自单独跑 18/18 全部通过——即失败只在两例同进程并发时出现；"
    "作者所述机理为「refresh_all 并发抓取，upsert_article_with_feed 的去重检查是读后写，故计数可为 0 或 2」，"
    "但代码不支持该解释：staged.rs 先 `let conn = db.lock().await;`（持锁）再调用 apply_refresh_result(&conn, ...)，"
    "其内部对 parsed.articles 的整轮 upsert 都在**同一次持锁**临界区内完成，"
    "故两个源的去重检查与插入被 Mutex 完全串行化、不可能交错，dedup_on 的计数不存在 0-or-2 的竞态窗口（应为确定的 1）；"
    "作者的解释也解释不了 dedup_off 例出现的 left:3（该例 dedup=false，根本不走去重分支），"
    "以及 dedup_on 例出现 0（0 意味着两源都没插入新文章，而非被去重掉一篇）。"
    "更贴合两种失败现象的候选机理是**同进程内两只用例的临时库路径撞车**："
    "refresh_dedup_e2e.rs:50-57 以 `format!(\"fluxreader_dedup_refresh_{}_{}.db\", process::id(), as_nanos())` 生成路径，"
    "同一测试二进制的两只用例 process::id() 相同，仅靠 SystemTime 纳秒区分（实测本机时钟粒度 100ns）；"
    "一旦两例在同一时基刻度内建库，它们会 `remove_file` 后打开**同一个** SQLite 文件，"
    "从而共享数据集——这恰好同时解释 on 例得 0（文章已被另一例预先插入而被去重）"
    "与 off 例得 3（自身两源各 1 篇 + 另一例留下的 1 篇）。"
    "该机理订正与修复应留给已记录的独立跟进项（建议改用每用例唯一路径或 `--test-threads=1`），"
    "不应由本任务承担，也不改变本任务结论。"
)

d["summary"] = d["summary"].replace(old_obs, new_obs)
assert new_obs in d["summary"], "summary patch failed"

for c in d["review_checks"]:
    if c["area"] == "failure_paths":
        c["analysis"] = c["analysis"].replace(
            "已披露 flake：我独立复现确认其为**真实存在**（见 summary 与 performance 观察），",
            "已披露 flake：我独立复现确认其为**真实存在**，且判定作者的机理说明不准确"
            "（staged.rs 在同一次持锁临界区内完成去重检查与插入，Mutex 已串行化两源，"
            "不存在作者所述的 0-or-2 读后写窗口；两种失败现象更符合同进程内临时库路径撞车，"
            "详见 summary），",
        )
        assert "同进程内临时库路径撞车" in c["analysis"], "failure_paths patch failed"

with io.open(p, "w", encoding="utf-8", newline="\n") as fh:
    json.dump(d, fh, ensure_ascii=False, indent=2)
    fh.write("\n")

raw = open(p, "rb").read()
print("BOM:", raw[:3] == b"\xef\xbb\xbf", "CRLF:", b"\r\n" in raw, "bytes:", len(raw))
