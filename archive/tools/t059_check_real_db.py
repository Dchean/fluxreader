"""只读检查用户真实数据库是否被 TASK-059 实机验证写入过。

做法：把 db/wal/shm **复制**到临时目录再打开（对原库零风险，不触发恢复/写入）。
SQL 全部为静态字符串且值走参数绑定；仅用于本次一次性核对。
"""

import os
import shutil
import sqlite3
import sys
import tempfile

REAL_DIR = r"C:\Users\A\AppData\Roaming\com.fluxreader.app"

SYNC_KEYS = (
    "sync_protocol",
    "greader_endpoint",
    "greader_username",
    "sync_last_sync",
    "sync_last_entry_id",
    "endpoint_resolved",
)


def main():
    src = os.path.join(REAL_DIR, "fluxreader.db")
    if not os.path.exists(src):
        print("真实数据库不存在：", src)
        return 0
    print("真实库路径:", src)
    for name in ("fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"):
        p = os.path.join(REAL_DIR, name)
        if os.path.exists(p):
            print(f"  {name}: size={os.path.getsize(p)}")

    tmpd = tempfile.mkdtemp(prefix="t059_realcheck_")
    for name in ("fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"):
        p = os.path.join(REAL_DIR, name)
        if os.path.exists(p):
            shutil.copy2(p, os.path.join(tmpd, name))

    con = sqlite3.connect(os.path.join(tmpd, "fluxreader.db"))
    cur = con.cursor()
    cur.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
    tables = {r[0] for r in cur.fetchall()}
    print("表:", sorted(tables))

    if "settings" in tables:
        cur.execute("SELECT COUNT(*) FROM settings")
        print("  settings count =", cur.fetchone()[0])
    if "feeds" in tables:
        cur.execute("SELECT COUNT(*) FROM feeds")
        print("  feeds count =", cur.fetchone()[0])
    if "folders" in tables:
        cur.execute("SELECT COUNT(*) FROM folders")
        print("  folders count =", cur.fetchone()[0])
    if "articles" in tables:
        cur.execute("SELECT COUNT(*) FROM articles")
        print("  articles count =", cur.fetchone()[0])

    if "settings" in tables:
        placeholders = ",".join("?" * len(SYNC_KEYS))
        cur.execute(
            f"SELECT key, substr(value,1,70) FROM settings WHERE key IN ({placeholders})",
            SYNC_KEYS,
        )
        rows = cur.fetchall()
        print("同步相关设置:", rows if rows else "（无）")

    if "feeds" in tables:
        cur.execute("SELECT id, title, feed_url FROM feeds LIMIT 10")
        print("feeds 前若干:", cur.fetchall())

    con.close()
    shutil.rmtree(tmpd, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())