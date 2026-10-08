# TASK-117（四阶段）/ TASK-128（媒体与布局负载补测）：真实规模数据注入——
# 备份 → 注入 → （测量后）还原。
#
# 纪律（沿 TASK-059 事故教训）：直接操作 %APPDATA%\com.fluxreader.app 的真实库；
# 任何写入前强制备份三件套并输出哈希；--restore 逐字节还原并复核哈希。
#
# 用法：
#   python tools/phase4_seed.py --info                       # 只读：当前库规模 + 按布局卡数 + 媒体覆盖
#   python tools/phase4_seed.py --seed 20000                 # 备份后注入 ~2 万篇文章
#   python tools/phase4_seed.py --seed 50000 --media --layouts
#                                                            # 追加真实媒体 URL + 五布局铺满（本卡新增）
#   python tools/phase4_seed.py --restore                    # 测量完成后还原真实库
#                                                           #（还原前必须完全退出应用；见 restore() 文档）
#   python tools/phase4_seed.py --restore --force            # 危险：应用仍在运行时强行还原（不建议）
#
# 写入的 DB 列（仅这些；其余列不碰）：
#   articles: feed_id, guid, title, author, url, url_norm?, summary, content_html,
#             body_text?, image_url?, enclosure_url?, enclosure_mime?, duration_sec?,
#             published_at, is_read, is_starred, created_at?, source?
#             （带 ? 的列只有在当前库确实存在时才写入——用 cols() 探测，不硬编码）
#   feeds:    layout（仅 --layouts：把布局轮转分配给现有订阅源）
#   folders:  layout（仅 --layouts 且需要新建分类时才写）
#   不写 settings / sync_queue / deduped_urls / 任何 AI 产物列。
import argparse, hashlib, json, os, random, shutil, sqlite3, subprocess, sys, time

APP_DIR = os.path.join(os.environ.get("APPDATA", ""), "com.fluxreader.app")
DB_FILES = ["fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"]
BACKUP_DIR = os.path.join("tmp", "phase4", "real-db-backup")

# 五种内容布局的 DB 取值（与 src/store/selectors.ts LAYOUT_NAMES 的键一致）：
#   article 文章 / social 社交 / image 画廊 / podcast 播客 / notification 通知
LAYOUTS = ["article", "social", "image", "podcast", "notification"]

# 稳定可用的公开媒体 URL（实机联网时才会真正拉取；可用命令行覆盖）：
#   图片：picsum.photos 按 seed 返回固定图，尺寸可控，无防盗链
#   音频：SoundHelix 公开示例 MP3（长期稳定；可用 --audio-url 换成本地/内网样本）
DEFAULT_IMAGE_URL = "https://picsum.photos/seed/{seed}/800/450"
DEFAULT_AUDIO_URL = "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3"

AUTHORS = ["林知行", "陈默", "苏晚", "Alex Rivera", "Priya Nair", "周牧", "Kenji Ito"]


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def db_path():
    p = os.path.join(APP_DIR, "fluxreader.db")
    if not os.path.exists(p):
        sys.exit(f"未找到真实库：{p}（确认应用安装并在本机运行过）")
    return p


def open_ro():
    """只读连接：只发 SELECT/PRAGMA，且用 PRAGMA query_only 从连接层禁止写入。"""
    conn = sqlite3.connect(db_path())
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA query_only=1")
    return conn


def cols(conn, table):
    return [r["name"] for r in conn.execute(f"PRAGMA table_info({table})")]


def effective_layout_sql():
    """布局口径与前端 resolveFeedLayout 一致：feed.layout 非 'inherit' 时覆盖分类布局。"""
    return "COALESCE(NULLIF(f.layout, 'inherit'), folder.layout, 'article')"


def layout_counts(conn):
    """按布局统计可见卡数（DB 口径：文章数按生效布局分组）。"""
    rows = conn.execute(
        f"""SELECT {effective_layout_sql()} AS eff, COUNT(a.id) AS n
            FROM articles a JOIN feeds f ON a.feed_id = f.id
            LEFT JOIN folders folder ON f.folder_id = folder.id
            GROUP BY eff"""
    ).fetchall()
    return {r["eff"]: r["n"] for r in rows}


def media_counts(conn):
    a_cols = cols(conn, "articles")

    def cnt(expr):
        return conn.execute(f"SELECT COUNT(*) FROM articles WHERE {expr}").fetchone()[0]
    out = {"articles": conn.execute("SELECT COUNT(*) FROM articles").fetchone()[0]}
    out["with_image_url"] = cnt("image_url IS NOT NULL AND image_url != ''") if "image_url" in a_cols else None
    out["with_enclosure_url"] = cnt("enclosure_url IS NOT NULL AND enclosure_url != ''") if "enclosure_url" in a_cols else None
    out["with_duration_sec"] = cnt("duration_sec IS NOT NULL") if "duration_sec" in a_cols else None
    out["with_body_text"] = cnt("body_text IS NOT NULL AND body_text != ''") if "body_text" in a_cols else None
    out["with_content_html"] = cnt("content_html IS NOT NULL AND content_html != ''") if "content_html" in a_cols else None
    return out


def feed_layout_map(conn):
    """{feed_id: (title, 生效布局)}——--info 与 --layouts 共用同一口径。"""
    rows = conn.execute(
        f"""SELECT f.id AS id, f.title AS title, {effective_layout_sql()} AS eff
            FROM feeds f LEFT JOIN folders folder ON f.folder_id = folder.id
            ORDER BY f.id"""
    ).fetchall()
    return {r["id"]: (r["title"], r["eff"]) for r in rows}


def info():
    conn = open_ro()
    arts = conn.execute("SELECT COUNT(*), SUM(is_read=0), SUM(is_starred=1) FROM articles").fetchone()
    feeds = conn.execute("SELECT COUNT(*) FROM feeds").fetchone()
    per_layout = layout_counts(conn)
    flm = feed_layout_map(conn)
    feeds_by_layout = {}
    for _fid, (_title, eff) in flm.items():
        feeds_by_layout[eff] = feeds_by_layout.get(eff, 0) + 1
    out = {
        "articles": arts[0],
        "unread": arts[1],
        "starred": arts[2],
        "feeds": feeds[0],
        "db_mb": round(os.path.getsize(db_path()) / 1048576, 1),
        # 与 UI 同口径：可见卡数按「feed 布局绑定 → 分类布局」解析后统计
        "articles_by_layout": {l: per_layout.get(l, 0) for l in LAYOUTS},
        "feeds_by_layout": {l: feeds_by_layout.get(l, 0) for l in LAYOUTS},
        "media_coverage": media_counts(conn),
        "layout_source": "feeds.layout（非 inherit 时覆盖）→ folders.layout，与前端 resolveFeedLayout 同口径",
    }
    missing = [l for l in LAYOUTS if per_layout.get(l, 0) == 0]
    if missing:
        out["layout_warning"] = f"以下布局当前 0 篇：{missing}（用 --seed N --layouts 铺满五种布局）"
    print(json.dumps(out, ensure_ascii=False, indent=1))


def backup(force=False):
    """备份三件套 + 写 SHA256 清单。

    保护「首次注入前的干净库」不被后续注入覆盖：若清单已存在、且当前库与清单不一致
    （说明库已被注入过），默认拒绝覆盖并要求先 --restore（或显式 --rebackup）。
    这样「跑第二次 --seed」不会再拿已注入的库当基线，--restore 才真正回到注入前。
    """
    os.makedirs(BACKUP_DIR, exist_ok=True)
    manifest_path = os.path.join(BACKUP_DIR, "manifest.json")
    if os.path.exists(manifest_path) and not force:
        with open(manifest_path) as f:
            old = json.load(f)
        same = True
        for name, want in old.items():
            src = os.path.join(APP_DIR, name)
            cur = sha256(src) if os.path.exists(src) else None
            if cur != want:
                same = False
                break
        if not same:
            sys.exit(
                "已存在一份「注入前」备份且当前库与它不一致（库看起来已被注入过）。\n"
                f"  备份位置：{BACKUP_DIR}\n"
                "  为免覆盖干净基线，本次不重新备份。请二选一：\n"
                "    1) 先还原：python tools/phase4_seed.py --restore，再重新 --seed；或\n"
                "    2) 确认当前库就是要作为新基线：python tools/phase4_seed.py --seed ... --rebackup"
            )
    manifest = {}
    for name in DB_FILES:
        src = os.path.join(APP_DIR, name)
        if os.path.exists(src):
            dst = os.path.join(BACKUP_DIR, name)
            shutil.copy2(src, dst)
            manifest[name] = sha256(dst)
        else:
            manifest[name] = None
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=1)
    print("已备份 →", BACKUP_DIR, json.dumps(manifest, indent=1))


def running_app_processes(app_name=None):
    """检测应用进程是否在运行：Windows 用 `tasklist`（无第三方依赖）。

    返回 (pids, detect_error)：
      - pids：匹配到的进程 PID 列表（空表示没在跑）；
      - detect_error：非 None 表示**无法判定**（非 Windows / tasklist 不可用），调用方应给警告，
        而不是把「查不到」当成「没在跑」。
    进程名可用环境变量 T128_APP_PROCESS 覆盖（与 tools/phase4_measure.mjs 同一口径，
    开发版 exe 名字不同时两边一起改）。
    """
    name = app_name or os.environ.get("T128_APP_PROCESS") or "FluxReader"
    image = name if name.lower().endswith(".exe") else name + ".exe"
    if os.name != "nt":
        return [], f"当前平台是 {os.name}，不做 Windows 进程检测"
    try:
        proc = subprocess.run(
            ["tasklist", "/FI", f"IMAGENAME eq {image}", "/NH", "/FO", "CSV"],
            capture_output=True, text=True, errors="replace", timeout=30,
        )
    except Exception as e:  # noqa: BLE001 - 检测失败要如实上报，不能静默通过
        return [], f"tasklist 执行失败：{e}"
    if proc.returncode != 0:
        return [], f"tasklist 返回码 {proc.returncode}：{(proc.stderr or '').strip()[:200]}"
    pids = []
    for line in (proc.stdout or "").splitlines():
        line = line.strip()
        if not line.startswith('"'):
            continue  # 「信息: 没有运行的任务匹配指定标准。」等本地化提示行
        fields = [f.strip().strip('"') for f in line.split('","')]
        if fields and fields[0].lower() == image.lower() and len(fields) > 1 and fields[1].isdigit():
            pids.append(int(fields[1]))
    return pids, None


def restore(force=False):
    """把备份的三件套（.db / -wal / -shm）逐字节拷回真实库。

    ⚠ 还原前必须**完全退出应用**（FluxReader.exe 进程结束）。
    为什么：应用运行时它的 SQLite 连接仍持有 .db/-wal/-shm 的句柄，逐字节覆盖可能被文件锁
    静默拒绝（旧文件原样保留）或在 WAL 恢复时把库改坏——而本函数末尾的 SHA256 只校验「拷过去的
    目标文件」，这种破坏下它必然通过，等于没有保护。

    因此：检测到应用进程仍在运行时**拒绝还原**（除非显式 force=True / --force）。
    检测不到（非 Windows、tasklist 不可用）时只打印警告，由操作者自行确认。
    """
    pids, detect_error = running_app_processes()
    if detect_error:
        print(f"⚠ 无法自动确认应用是否已退出（{detect_error}）。")
        print("  还原前请自行确认 FluxReader 已完全退出（否则运行中的 SQLite 连接可能让覆盖静默失效或损坏库）。")
    elif pids:
        if not force:
            sys.exit(
                "检测到 FluxReader 仍在运行（PID: " + ", ".join(str(p) for p in pids) + "）：拒绝还原。\n"
                "  原因：应用的活动 SQLite 连接持有 .db/-wal/-shm 句柄，运行中逐字节覆盖可能静默失效\n"
                "        甚至损坏真实库；而还原结束时的 SHA256 只校验目标文件，这种破坏下必然通过。\n"
                "  处置：完全退出 FluxReader（关掉窗口后确认进程结束）再重跑 --restore。\n"
                "  确认进程是否真的没了：tasklist /FI \"IMAGENAME eq FluxReader.exe\"\n"
                "  确实要在应用运行时还原（危险，不建议）：加 --force。"
            )
        print("⚠ --force：应用仍在运行（PID: " + ", ".join(str(p) for p in pids) + "）仍继续还原——可能静默失效或损坏库，风险自负。")
    with open(os.path.join(BACKUP_DIR, "manifest.json")) as f:
        manifest = json.load(f)
    for name, want in manifest.items():
        dst = os.path.join(APP_DIR, name)
        if want is None:
            if os.path.exists(dst):
                os.remove(dst)
            continue
        shutil.copy2(os.path.join(BACKUP_DIR, name), dst)
        got = sha256(dst)
        if got != want:
            sys.exit(f"还原哈希不一致：{name} {got} != {want}——不要继续，检查杀软/云同步干扰")
    print("已逐字节还原并校验通过。")


# ============================================================
# --layouts：把五种布局铺满
# ============================================================

SEED_FOLDER_NAME = "phase4-seed-layouts"


def assign_layouts(conn, create_missing=True):
    """把布局轮转分配给订阅源，保证五种布局各至少有一个源。

    - 现有源 ≥5：按 id 顺序轮转改写 feeds.layout（只写这一列）。
    - 现有源 <5 且 create_missing：补建一个名为 phase4-seed-layouts 的分类，
      在其中补建缺口数量的「种子源」（feed_url 用 https://seed.invalid/...，
      不可解析——应用刷新时它们会明确失败，还原后随备份一起消失）。
    返回 (layout→[feed_id], 改动说明列表)。
    """
    rows = conn.execute("SELECT id, title FROM feeds ORDER BY id").fetchall()
    changes = []
    by_layout = {l: [] for l in LAYOUTS}
    if len(rows) >= len(LAYOUTS):
        for i, r in enumerate(rows):
            layout = LAYOUTS[i % len(LAYOUTS)]
            by_layout[layout].append(r["id"])
            cur = conn.execute("SELECT layout FROM feeds WHERE id=?", (r["id"],)).fetchone()["layout"]
            if cur != layout:
                conn.execute("UPDATE feeds SET layout=? WHERE id=?", (layout, r["id"]))
                changes.append(f"feeds#{r['id']} layout {cur} → {layout}")
        return by_layout, changes

    # 源不足：现有源全部铺开，缺口用种子分类里的新源补齐
    folder_id = None
    if create_missing:
        f_cols = cols(conn, "folders")
        existing = conn.execute("SELECT id FROM folders WHERE name=?", (SEED_FOLDER_NAME,)).fetchone()
        if existing:
            folder_id = existing["id"]
        else:
            vals = {"name": SEED_FOLDER_NAME}
            use = [c for c in f_cols if c in vals]
            if "layout" in f_cols:
                use.append("layout"); vals["layout"] = "article"
            if "created_at" in f_cols:
                use.append("created_at"); vals["created_at"] = time.strftime("%Y-%m-%dT%H:%M:%S+08:00")
            ph = ",".join("?" * len(use))
            cur = conn.execute(f"INSERT INTO folders ({','.join(use)}) VALUES ({ph})", [vals[c] for c in use])
            folder_id = cur.lastrowid
            changes.append(f"新建分类 {SEED_FOLDER_NAME} (id={folder_id})")

    fd_cols = cols(conn, "feeds")
    for i, r in enumerate(rows):
        layout = LAYOUTS[i % len(LAYOUTS)]
        by_layout[layout].append(r["id"])
        conn.execute("UPDATE feeds SET layout=? WHERE id=?", (layout, r["id"]))
        changes.append(f"feeds#{r['id']} layout → {layout}")

    need = [l for l in LAYOUTS if not by_layout[l]]
    for layout in need:
        url = f"https://seed.invalid/phase4/{layout}.xml"
        # 幂等：上一轮注入留下的同名种子源（feeds.feed_url 是 UNIQUE）直接复用，
        # 否则同一条命令跑第二次会撞唯一约束。
        found = conn.execute("SELECT id FROM feeds WHERE feed_url=?", (url,)).fetchone()
        if found:
            by_layout[layout].append(found["id"])
            conn.execute("UPDATE feeds SET layout=? WHERE id=?", (layout, found["id"]))
            changes.append(f"复用已有种子源 feeds#{found['id']} layout={layout} url={url}")
            continue
        vals = {
            "feed_url": url, "title": f"phase4 种子源（{layout}）", "site_url": url,
            "folder_id": folder_id, "layout": layout, "origin": "local",
        }
        use = [c for c in fd_cols if c in vals]
        ph = ",".join("?" * len(use))
        cur = conn.execute(f"INSERT INTO feeds ({','.join(use)}) VALUES ({ph})", [vals[c] for c in use])
        by_layout[layout].append(cur.lastrowid)
        changes.append(f"新建种子源 feeds#{cur.lastrowid} layout={layout} url={url}")
    return by_layout, changes


# ============================================================
# --seed：注入文章
# ============================================================

def seed(total, media=False, layouts=False, media_ratio=0.6, rng_seed=20261007,
         image_url_tpl=DEFAULT_IMAGE_URL, audio_url=DEFAULT_AUDIO_URL):
    random.seed(rng_seed)
    conn = sqlite3.connect(db_path())
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA query_only=0")
    a_cols = cols(conn, "articles")
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("BEGIN")
    changes = []
    try:
        if layouts:
            layout_feeds, changes = assign_layouts(conn)
            for line in changes:
                print("  [layouts]", line)
        else:
            layout_feeds = {None: [r["id"] for r in conn.execute("SELECT id FROM feeds").fetchall()]}
        feed_rows = [r["id"] for r in conn.execute("SELECT id FROM feeds").fetchall()]
        if not feed_rows:
            conn.rollback()
            sys.exit("无订阅源可挂靠——先在应用里加至少 1 个订阅源再注入")
        # 每个布局都至少有一个源时才按布局铺；否则回落到「全部源随机」
        plan_layouts = [l for l in LAYOUTS if layout_feeds.get(l)] if layouts else [None]

        titles = {
            "article": ["性能测量注入文章", "大库滚动基线样本", "查询耗时样本"],
            "social": ["社交布局减压样本", "时间线条目样本"],
            "image": ["画廊布局图片样本", "摄影流样本"],
            "podcast": ["播客布局音频样本", "长音频解码样本"],
            "notification": ["通知布局摘要样本", "轻量文本样本"],
        }
        now = int(time.time())
        media_n = 0
        inserted = 0
        for i in range(total):
            layout = plan_layouts[i % len(plan_layouts)]
            pool = layout_feeds.get(layout) if layout else feed_rows
            feed = random.choice(pool)
            title = f"{random.choice(titles.get(layout) or titles['article'])} #{i:06d}"
            # 布局决定正文形态：
            #   社交=长 HTML 正文（含 <img>，制造真实解码与折叠判定压力）
            #   画廊=以 image_url 为主（封面即内容）
            #   播客=enclosure_url + duration_sec（真实音频引用）
            #   文章/通知=多段落 HTML
            img_seed = f"p4-{i}"
            img = image_url_tpl.format(seed=img_seed)
            has_media = media and (random.random() < media_ratio)
            if layout == "image" and media:
                has_media = True  # 画廊布局必带图，否则等于无内容
            if layout == "podcast" and media:
                has_media = True
            paras = [f"<p>样本段落 {i}-{k}：用于大库滚动与查询测量的合成内容，长度足够产生真实渲染成本。</p>" for k in range(3)]
            if layout == "social":
                body = "".join(paras)
                if has_media:
                    body += f'<p><img src="{img}" alt="sample-{i}"></p>'
            elif layout == "image":
                body = f'<p><img src="{img}" alt="gallery-{i}"></p>' if has_media else "<p>无图样本</p>"
            elif layout == "podcast":
                body = f"<p>播客节目 {i} 的节目说明。</p>"
            else:
                body = "".join(paras)[:3000]
            rfc = time.strftime('%Y-%m-%dT%H:%M:%S+08:00', time.gmtime(now - i * 60 + 8 * 3600))
            snippet_text = "".join(f"样本段落 {i}-{k}：" for k in range(3)) + "用于大库滚动与查询测量的合成内容。"
            vals = {
                "feed_id": feed, "title": title, "content_html": body, "snippet": title,
                "summary": snippet_text[:280],
                "body_text": "".join(p.replace("<p>", "").replace("</p>", " ") for p in paras),
                "author": random.choice(AUTHORS) if media else "phase4-seed",
                "url": f"https://seed.invalid/article/{i}",
                "published_at": rfc, "is_read": random.random() < 0.7,
                "is_starred": random.random() < 0.05,
            }
            if has_media:
                vals["image_url"] = img
                media_n += 1
                if layout == "podcast":
                    vals["enclosure_url"] = audio_url
                    vals["enclosure_mime"] = "audio/mpeg"
                    vals["duration_sec"] = random.randint(180, 5400)
            use = [c for c in a_cols if c in vals]
            if "guid" in a_cols and "guid" not in use:
                use.append("guid"); vals["guid"] = f"seed-{i}"
            if "created_at" in a_cols and "created_at" not in use:
                use.append("created_at"); vals["created_at"] = rfc
            if "url_norm" in a_cols and "url_norm" not in use:
                use.append("url_norm"); vals["url_norm"] = f"https://seed.invalid/article/{i}"
            if "source" in a_cols and "source" not in use:
                use.append("source"); vals["source"] = "direct"
            ph = ",".join("?" * len(use))
            cur = conn.execute(f"INSERT OR IGNORE INTO articles ({','.join(use)}) VALUES ({ph})", [vals[c] for c in use])
            inserted += cur.rowcount if cur.rowcount and cur.rowcount > 0 else 0
            if i % 2000 == 0:
                print(f"  {i}/{total}")
        conn.commit()
    except Exception as e:
        conn.rollback()
        sys.exit(f"注入失败已回滚（真实库未受损）：{e}")

    n = conn.execute("SELECT COUNT(*) FROM articles").fetchone()[0]
    per_layout = layout_counts(conn)
    skipped = total - inserted
    print(f"注入完成：本轮新增 {inserted} 篇，跳过 {skipped} 篇（guid 已存在），当前文章总数 {n}（本轮带媒体 {media_n} 篇）。")
    if skipped:
        print("  提示：跳过说明这个库已经有上一轮注入的行——要干净基线请先 --restore 再重新 --seed。")
    print("  按布局：" + json.dumps({l: per_layout.get(l, 0) for l in LAYOUTS}, ensure_ascii=False))
    empty = [l for l in LAYOUTS if per_layout.get(l, 0) == 0]
    if layouts and empty:
        print(f"  ⚠ 仍有布局无卡：{empty}——检查该布局的源是否被删除/禁用")
    if layouts and changes:
        print("  布局写入明细（--restore 会一并还原）：")
        for line in changes:
            print("   ", line)
    print("测量完成后务必：python tools/phase4_seed.py --restore")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--info", action="store_true")
    ap.add_argument("--seed", type=int)
    ap.add_argument("--restore", action="store_true")
    ap.add_argument("--media", action="store_true",
                    help="为部分种子行写入真实媒体 URL（image_url / author / duration_sec / enclosure_url）")
    ap.add_argument("--media-ratio", type=float, default=0.6,
                    help="带媒体行占比（默认 0.6；画廊/播客布局的种子行恒带媒体）")
    ap.add_argument("--layouts", action="store_true",
                    help="把五种内容布局都铺上种子卡（轮转改写 feeds.layout；源不足时补建种子分类/源）")
    ap.add_argument("--image-url", default=DEFAULT_IMAGE_URL,
                    help="图片 URL 模板，{seed} 会被替换（默认 picsum.photos）")
    ap.add_argument("--audio-url", default=DEFAULT_AUDIO_URL,
                    help="播客音频 URL（默认 SoundHelix 公开示例）")
    ap.add_argument("--rng-seed", type=int, default=20261007, help="随机种子，保证多次注入同构（默认 20261007）")
    ap.add_argument("--rebackup", action="store_true",
                    help="允许覆盖已有的「注入前」备份（默认拒绝，防止把已注入的库当基线）")
    ap.add_argument("--force", action="store_true",
                    help="危险：--restore 时即使检测到应用仍在运行也继续（应用活动连接下覆盖 .db 可能静默失效或损坏库）")
    a = ap.parse_args()
    if a.info:
        info()
    elif a.restore:
        restore(force=a.force)
    elif a.seed:
        pids, detect_error = running_app_processes()
        if pids:
            print("⚠ 检测到 FluxReader 正在运行（PID: " + ", ".join(str(p) for p in pids) + "）：注入会在应用的活动连接旁写库，"
                  "应用可能读不到新行或与注入互相阻塞。建议先完全退出应用再注入（本脚本不阻止，因为读库方是它自己）。")
        elif detect_error:
            print(f"⚠ 无法自动确认应用是否已退出（{detect_error}）——注入前请自行确认。")
        print("备份真实库…"); backup(force=a.rebackup)
        seed(a.seed, media=a.media, layouts=a.layouts, media_ratio=a.media_ratio,
             rng_seed=a.rng_seed, image_url_tpl=a.image_url, audio_url=a.audio_url)
    else:
        ap.print_help()


if __name__ == "__main__":
    main()
