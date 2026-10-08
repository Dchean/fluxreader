"""R1 验收探针（临时，不属于候选）：在沙箱目录里端到端验证 phase4_seed.restore() 的三条路径。
不触碰真实库：把模块的 APP_DIR / BACKUP_DIR 指到 tmp 下的假目录。"""
import hashlib
import importlib.util
import os
import shutil
import sys

ROOT = "tmp/phase4/restore-sandbox"
FAKE_APP = os.path.join(ROOT, "app")
FAKE_BACKUP = os.path.join(ROOT, "backup")

spec = importlib.util.spec_from_file_location("seed", "tools/phase4_seed.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)

m.APP_DIR = FAKE_APP
m.BACKUP_DIR = FAKE_BACKUP
m.DB_FILES = ["fluxreader.db", "fluxreader.db-wal", "fluxreader.db-shm"]


def setup():
    shutil.rmtree(ROOT, ignore_errors=True)
    os.makedirs(FAKE_APP, exist_ok=True)
    os.makedirs(FAKE_BACKUP, exist_ok=True)
    for name, data in [("fluxreader.db", b"BACKUP-DB"), ("fluxreader.db-wal", b"BACKUP-WAL"), ("fluxreader.db-shm", b"BACKUP-SHM")]:
        with open(os.path.join(FAKE_BACKUP, name), "wb") as f:
            f.write(data)
    for name in m.DB_FILES:
        with open(os.path.join(FAKE_APP, name), "wb") as f:
            f.write(b"INJECTED-XXXX")
    manifest = {name: hashlib.sha256(open(os.path.join(FAKE_BACKUP, name), "rb").read()).hexdigest() for name in m.DB_FILES}
    import json
    with open(os.path.join(FAKE_BACKUP, "manifest.json"), "w") as f:
        json.dump(manifest, f)


def read(name):
    p = os.path.join(FAKE_APP, name)
    return open(p, "rb").read().decode() if os.path.exists(p) else None


def case(label, pids, detect_error, force):
    setup()
    m.running_app_processes = lambda app_name=None: (pids, detect_error)
    print(f"--- {label} ---")
    try:
        m.restore(force=force)
        outcome = "returned"
    except SystemExit as e:
        outcome = f"SystemExit: {str(e).splitlines()[0]}"
    print("outcome:", outcome)
    print("app dir after:", {n: read(n) for n in m.DB_FILES})


case("① 应用在跑 + 无 --force → 必须拒绝且不动文件", [4242], None, False)
case("② 应用在跑 + --force → 继续还原并校验", [4242], None, True)
case("③ 应用没在跑 → 正常还原", [], None, False)
case("④ 无法判定（检测失败）→ 警告后继续", [], "tasklist 不可用（模拟）", False)
