"""从 WAL 帧中恢复 settings 表的历史版本（只读副本，绝不触碰真实库）。

背景：TASK-059 的实机验证误写用户真实库（Tauri 的 app_data_dir 走 Win32 已知文件夹
API，不受 APPDATA 环境变量影响）。`sync_save` 检测到「换账号」后还执行了
`purge_remote_data`。本脚本用于**还原**：把 WAL 里每一帧的 settings 行解析出来，
按帧序打印历史版本，从而取回测试之前的那一份配置。

只解析 `-wal` 的副本；不写任何文件。
"""

import os
import sys

WAL = os.environ.get("T059_WAL", r"C:\Users\A\AppData\Local\Temp\t059-incident\fluxreader.db-wal")
DB = os.environ.get("T059_DB", r"C:\Users\A\AppData\Local\Temp\t059-incident\fluxreader.db")

WATCH = ("sync_protocol", "greader_endpoint", "greader_username", "greader_password",
         "sync_last_sync", "sync_last_entry_id", "endpoint_resolved")


def read_varint(buf, off):
    val = 0
    for i in range(9):
        b = buf[off + i]
        if i == 8:
            val = (val << 8) | b
            return val, off + 9
        val = (val << 7) | (b & 0x7F)
        if not (b & 0x80):
            return val, off + 1
    raise ValueError("varint too long")


def serial_len(stype):
    if stype >= 13 and stype % 2 == 1:
        return (stype - 13) // 2, "text"
    if stype >= 12 and stype % 2 == 0:
        return (stype - 12) // 2, "blob"
    return {0: (0, "null"), 1: (1, "int"), 2: (2, "int"), 3: (3, "int"),
            4: (4, "int"), 5: (6, "int"), 6: (8, "int"), 8: (0, "int0"),
            9: (0, "int1")}.get(stype, (0, "other"))


def decode_leaf(page):
    """解析表叶子页，返回 [(key, value)]（仅取前两列，即 settings 的 key/value）。"""
    rows = []
    if not page or page[0] != 0x0D:
        return rows
    ncell = int.from_bytes(page[3:5], "big")
    for i in range(ncell):
        cell_off = int.from_bytes(page[8 + 2 * i: 10 + 2 * i], "big")
        if cell_off == 0 or cell_off >= len(page):
            continue
        payload, off = read_varint(page, cell_off)
        _rowid, off = read_varint(page, off)
        # 记录头：header-size varint 的**取值**包含它自身的字节数，
        # 故头部区间是 [off, off + hdr_size)，值从 off + hdr_size 开始。
        hdr_size, off2 = read_varint(page, off)
        hdr_end = off + hdr_size
        types = []
        p = off2
        while p < hdr_end:
            t, p = read_varint(page, p)
            types.append(t)
        body = hdr_end
        vals = []
        for t in types[:2]:
            ln, kind = serial_len(t)
            raw = page[body:body + ln]
            body += ln
            if kind == "text":
                vals.append(raw.decode("utf-8", "replace"))
            elif kind == "blob":
                vals.append(f"<blob {len(raw)}>")
            else:
                vals.append(int.from_bytes(raw, "big") if raw else 0)
        if len(vals) == 2:
            rows.append((vals[0], vals[1]))
    return rows


def main():
    with open(DB, "rb") as fh:
        dbhdr = fh.read(100)
    page_size = int.from_bytes(dbhdr[16:18], "big")
    if page_size == 1:
        page_size = 65536
    print("page_size =", page_size)

    with open(WAL, "rb") as fh:
        wal = fh.read()
    magic = int.from_bytes(wal[0:4], "big")
    wal_page = int.from_bytes(wal[8:12], "big")
    print(f"wal magic=0x{magic:08x} page_size={wal_page} frames≈{(len(wal)-32)//(24+wal_page)}")

    # settings 表根页
    import sqlite3, shutil, tempfile
    tmpd = tempfile.mkdtemp(prefix="t059_walread_")
    for n in ("fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"):
        src = os.path.join(os.path.dirname(DB), n)
        if os.path.exists(src):
            shutil.copy2(src, os.path.join(tmpd, n))
    con = sqlite3.connect(os.path.join(tmpd, "fluxreader.db"))
    root = con.execute("SELECT rootpage FROM sqlite_master WHERE name='settings'").fetchone()[0]
    con.close()
    shutil.rmtree(tmpd, ignore_errors=True)
    print("settings rootpage =", root)

    versions = []
    off = 32
    idx = 0
    while off + 24 + wal_page <= len(wal):
        pageno = int.from_bytes(wal[off:off + 4], "big")
        dbsize = int.from_bytes(wal[off + 4:off + 8], "big")
        if pageno == root:
            page = wal[off + 24: off + 24 + wal_page]
            rows = decode_leaf(page)
            rec = {k: v for k, v in rows if isinstance(k, str)}
            versions.append((idx, dbsize != 0, {k: rec.get(k) for k in WATCH if k in rec}))
        off += 24 + wal_page
        idx += 1

    print(f"\nsettings 页共出现 {len(versions)} 帧（仅列触碰过待观察键的帧）:\n")
    touching = [(i, c, r) for i, c, r in versions if r]

    # 定位「测试接管」的那一刻：用户真实配置（reader.miniflux.app）
    # 最后一次出现，与本地测试端点（127.0.0.1）首次出现的帧号。
    real_frames = [
        (i, r)
        for i, _c, r in touching
        if "miniflux" in str(r.get("greader_endpoint"))
        or "miniflux" in str(r.get("greader_password"))
    ]
    if real_frames:
        last_i, last_r = real_frames[-1]
        print(f"*** 用户真实配置最后出现于 frame {last_i} ***")
        for k, v in last_r.items():
            print(f"    {k} = {v!r}")
    for i, _c, r in touching:
        if "127.0.0.1" in str(r.get("greader_endpoint")):
            print(f"*** 本地测试端点首次出现于 frame {i} ***")
            break

    tail = int(os.environ.get("T059_TAIL", "0") or 0)
    rng_from = int(os.environ.get("T059_FROM", "0") or 0)
    rng_to = int(os.environ.get("T059_TO", "0") or 0)
    if tail:
        for i, committed, rec in touching[-tail:]:
            print(f"--- frame {i} committed={committed} ---")
            for k, v in rec.items():
                show = v if not isinstance(v, str) or len(v) <= 90 else v[:90] + "…"
                print(f"    {k} = {show!r}")
    if rng_to:
        for i, committed, rec in touching:
            if rng_from <= i <= rng_to:
                print(f"--- frame {i} committed={committed} ---")
                for k, v in rec.items():
                    show = v if not isinstance(v, str) or len(v) <= 90 else v[:90] + "…"
                    print(f"    {k} = {show!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
