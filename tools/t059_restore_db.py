"""把 WAL 帧重放到「测试接管之前」的那次提交，重建用户原库状态。

为什么需要：TASK-059 的实机验证误写了用户真实库（Tauri 的 app_data_dir 走 Win32
已知文件夹 API，`APPDATA` 环境变量无效），且 `sync_save` 的「换账号」检测还触发了
`purge_remote_data`。所幸 WAL 保留了全部历史帧，可按帧序重放出**测试之前**的状态。

本脚本只读源文件，把还原结果写到**新建立的临时文件**（路径由 tempfile 生成）；
是否采用、如何落回，由调用方另行决定。

用法：
    python tools/t059_restore_db.py --db <库> --wal <wal> --stop-before 77
"""

import argparse
import os
import sqlite3
import sys
import tempfile

MAGIC_BIG = 0x377F0682
MAGIC_LITTLE = 0x377F0683


def replay(db_path, wal_path, stop_before):
    """按 WAL 帧序重放到 stop_before 之前的最后一次提交，返回还原后的库字节。"""
    with open(db_path, "rb") as fh:
        base = fh.read()
    with open(wal_path, "rb") as fh:
        wal = fh.read()

    magic = int.from_bytes(wal[0:4], "big")
    if magic not in (MAGIC_BIG, MAGIC_LITTLE):
        raise SystemExit("WAL magic 非法: 0x%08x" % magic)
    page_size = int.from_bytes(wal[8:12], "big") or 4096
    header_page_size = int.from_bytes(base[16:18], "big")
    if header_page_size == 1:
        header_page_size = 65536
    if header_page_size != page_size:
        raise SystemExit("页大小不一致：db=%d wal=%d" % (header_page_size, page_size))

    frame_size = 24 + page_size
    total_frames = (len(wal) - 32) // frame_size
    print("页大小 %d，WAL 帧数 %d，停帧 < %d" % (page_size, total_frames, stop_before))

    last_commit = None
    for i in range(min(stop_before, total_frames)):
        off = 32 + i * frame_size
        dbsize = int.from_bytes(wal[off + 4:off + 8], "big")
        if dbsize != 0:
            last_commit = (i, dbsize)
    if last_commit is None:
        raise SystemExit("在该范围找不到提交帧；无法安全还原")
    ci, csize = last_commit
    print("最后一次提交：frame %d，db 页数 = %d" % (ci, csize))

    pages = {}
    for i in range(ci + 1):
        off = 32 + i * frame_size
        pageno = int.from_bytes(wal[off:off + 4], "big")
        pages[pageno] = wal[off + 24: off + 24 + page_size]
    print("重放 %d 帧，覆盖 %d 个页" % (ci + 1, len(pages)))

    out = bytearray(base)
    need = csize * page_size
    if len(out) < need:
        out.extend(b"\x00" * (need - len(out)))
    else:
        del out[need:]
    for pageno, img in pages.items():
        if pageno == 0 or pageno > csize:
            continue
        start = (pageno - 1) * page_size
        out[start:start + page_size] = img
    out[28:32] = csize.to_bytes(4, "big")
    return bytes(out)


def verify(path):
    """打开并打印要点，确认还原结果（对象是自建临时文件）。"""
    con = sqlite3.connect(path)
    cur = con.cursor()
    cur.execute("PRAGMA integrity_check")
    print("integrity_check:", cur.fetchone()[0])

    cur.execute("SELECT COUNT(*) FROM feeds")
    print("feeds =", cur.fetchone()[0])
    cur.execute("SELECT COUNT(*) FROM articles")
    print("articles =", cur.fetchone()[0])
    cur.execute("SELECT COUNT(*) FROM folders")
    print("folders =", cur.fetchone()[0])
    cur.execute("SELECT COUNT(*) FROM sync_queue")
    print("sync_queue =", cur.fetchone()[0])

    cur.execute("SELECT key, value FROM settings")
    for key, value in cur.fetchall():
        text = str(value)
        shown = text if len(text) <= 70 else text[:70] + "…"
        print("  setting %r = %r" % (key, shown))

    cur.execute("SELECT id, title FROM feeds LIMIT 10")
    print("feeds 明细:", cur.fetchall())
    con.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--wal", required=True)
    ap.add_argument("--stop-before", type=int, required=True)
    args = ap.parse_args()

    restored = replay(args.db, args.wal, args.stop_before)
    fd, outfile = tempfile.mkstemp(prefix="t059_restored_", suffix=".db")
    with os.fdopen(fd, "wb") as fh:
        fh.write(restored)
    print("已写出还原库：%s（%d 字节）" % (outfile, len(restored)))
    verify(outfile)
    return 0


if __name__ == "__main__":
    sys.exit(main())