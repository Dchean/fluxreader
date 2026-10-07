# TASK-117（四阶段）：真实规模数据注入——备份 → 注入 → （测量后）还原。
# 纪律（沿 TASK-059 事故教训）：直接操作 %APPDATA%\com.fluxreader.app 的真实库；
# 任何写入前强制备份三件套并输出哈希；--restore 逐字节还原并复核哈希。
# 用法：
#   python tools/phase4_seed.py --info                 # 只读：报告当前库规模
#   python tools/phase4_seed.py --seed 20000           # 备份后注入 ~2 万篇文章
#   python tools/phase4_seed.py --restore              # 测量完成后还原真实库
import argparse, hashlib, json, os, random, shutil, sqlite3, sys, time

APP_DIR = os.path.join(os.environ.get("APPDATA", ""), "com.fluxreader.app")
DB_FILES = ["fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"]
BACKUP_DIR = os.path.join("tmp", "phase4", "real-db-backup")


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
    conn = sqlite3.connect(db_path())
    conn.row_factory = sqlite3.Row
    return conn


def cols(conn, table):
    return [r["name"] for r in conn.execute(f"PRAGMA table_info({table})")]


def info():
    conn = open_ro()
    arts = conn.execute("SELECT COUNT(*), SUM(is_read=0), SUM(is_starred=1) FROM articles").fetchone()
    feeds = conn.execute("SELECT COUNT(*) FROM feeds").fetchone()
    print(json.dumps({"articles": arts[0], "unread": arts[1], "starred": arts[2],
                      "feeds": feeds[0], "db_mb": round(os.path.getsize(db_path()) / 1048576, 1)}, ensure_ascii=False))


def backup():
    os.makedirs(BACKUP_DIR, exist_ok=True)
    manifest = {}
    for name in DB_FILES:
        src = os.path.join(APP_DIR, name)
        if os.path.exists(src):
            dst = os.path.join(BACKUP_DIR, name)
            shutil.copy2(src, dst)
            manifest[name] = sha256(dst)
        else:
            manifest[name] = None
    with open(os.path.join(BACKUP_DIR, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1)
    print("已备份 →", BACKUP_DIR, json.dumps(manifest, indent=1))


def restore():
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


def seed(total):
    conn = sqlite3.connect(db_path())
    conn.row_factory = sqlite3.Row
    a_cols, f_cols = cols(conn, "articles"), cols(conn, "feeds")
    feed_rows = conn.execute("SELECT id FROM feeds").fetchall()
    if not feed_rows:
        sys.exit("无订阅源可挂靠——先在应用里加至少 1 个订阅源再注入")
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("BEGIN")
    titles = ["性能测量注入文章", "大库滚动基线样本", "查询耗时样本", "媒体布局样本", "同步压力样本"]
    bodies = ["<p>%s</p>" * 3] * 2
    now = int(time.time())
    try:
        for i in range(total):
            feed = random.choice(feed_rows)["id"]
            title = f"{random.choice(titles)} #{i:06d}"
            body = "".join(f"<p>样本段落 {i}-{k}：用于大库滚动与查询测量的合成内容，长度足够产生真实渲染成本。</p>" for k in range(3))
            vals = {"feed_id": feed, "title": title, "content_html": body, "snippet": title,
                    "author": "phase4-seed", "url": f"https://seed.local/article/{i}",
                    "published_at": now - i * 60, "is_read": random.random() < 0.7,
                    "is_starred": random.random() < 0.05}
            use = [c for c in a_cols if c in vals]
            if "guid" in a_cols and "guid" not in use:
                use.append("guid"); vals["guid"] = f"seed-{i}"
            if "created_at" in a_cols and "created_at" not in use:
                use.append("created_at"); vals["created_at"] = now - i * 60
            ph = ",".join("?" * len(use))
            conn.execute(f"INSERT INTO articles ({','.join(use)}) VALUES ({ph})", [vals[c] for c in use])
            if i % 2000 == 0:
                print(f"  {i}/{total}")
        conn.commit()
    except Exception as e:
        conn.rollback()
        sys.exit(f"注入失败已回滚（真实库未受损）：{e}")
    n = conn.execute("SELECT COUNT(*) FROM articles").fetchone()[0]
    print(f"注入完成，当前文章总数 {n}。测量完成后务必：python tools/phase4_seed.py --restore")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--info", action="store_true")
    ap.add_argument("--seed", type=int)
    ap.add_argument("--restore", action="store_true")
    a = ap.parse_args()
    if a.info:
        info()
    elif a.restore:
        restore()
    elif a.seed:
        print("备份真实库…"); backup()
        seed(a.seed)
    else:
        ap.print_help()


if __name__ == "__main__":
    main()
