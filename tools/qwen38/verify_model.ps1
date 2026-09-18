# Verify downloaded snapshot: revision pin + crc32.txt integrity + size manifest
$ErrorActionPreference = 'Continue'
$py = @'
import os, sys, zlib, json
root = r"C:\ai\models\qwen38\hf"
# 1) revision check from hf metadata
rev = None
metadir = os.path.join(root, ".cache", "huggingface")
for base in (os.path.join(root, ".huggingface"), metadir):
    pass
# snapshot_download(local_dir=...) writes .cache/huggingface/.gitignore etc; authoritative: re-resolve via API
from huggingface_hub import HfApi
info = HfApi().model_info("Qwen/Qwen3.8-27B", revision="1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0", files_metadata=True)
print("REMOTE_SHA", info.sha)
sizes = {s.rfilename: s.size for s in info.siblings}
lfs = {s.rfilename: (s.lfs.sha256 if s.lfs else None) for s in info.siblings}
# 2) local files vs remote size manifest
bad = 0
for name, sz in sorted(sizes.items()):
    p = os.path.join(root, name.replace("/", os.sep))
    if not os.path.exists(p):
        print("MISSING", name); bad += 1; continue
    actual = os.path.getsize(p)
    if sz is not None and actual != sz:
        print("SIZE_MISMATCH", name, actual, sz); bad += 1
print("SIZE_CHECK", "PASS" if bad == 0 else f"FAIL({bad})", "files", len(sizes))
# 3) crc32.txt verification
crcpath = os.path.join(root, "crc32.txt")
if os.path.exists(crcpath):
    entries = []
    for line in open(crcpath, encoding="utf-8"):
        line = line.strip()
        if not line: continue
        parts = line.split()
        entries.append(parts)
    print("CRC_ENTRIES", len(entries), entries[:2])
    crcbad = 0
    for parts in entries:
        # accept "name crc" or "crc name" orders
        if len(parts) < 2: continue
        a, b = parts[0], parts[-1]
        name, want = (a, b) if os.path.exists(os.path.join(root, a)) else (b, a)
        p = os.path.join(root, name)
        if not os.path.exists(p):
            print("CRC_FILE_MISSING", name); crcbad += 1; continue
        c = 0
        with open(p, "rb") as f:
            while True:
                chunk = f.read(1 << 22)
                if not chunk: break
                c = zlib.crc32(chunk, c)
        got = format(c & 0xFFFFFFFF, "08x")
        if got.lower() != want.lower().replace("0x", "").zfill(8):
            print("CRC_MISMATCH", name, got, want); crcbad += 1
    print("CRC_CHECK", "PASS" if crcbad == 0 else f"FAIL({crcbad})")
else:
    print("CRC_CHECK", "NO_CRC_FILE")
'@
Set-Content -Path C:\ai\models\qwen38\verify.py -Value $py -Encoding UTF8
python C:\ai\models\qwen38\verify.py
