#!/usr/bin/env python3
"""Pack misaki's IPA lexicon into a binary the Rust G2P can binary-search.

misaki's `us_gold.json` already speaks Kokoro's own symbol set, so no ARPAbet
conversion is needed. Values are either a string or a POS-keyed dict; we take
`DEFAULT`.

    python3 build_lexicon.py us_gold.json ../data/us_lexicon.bin

Format (little endian), entries sorted by key bytes:

    magic  b"MKLEX\\0\\0\\0"   8 bytes
    count  u32
    index  count * { key_off u32, val_off u32 }   offsets into blob
    blob   NUL-terminated utf8 strings
"""

import json
import struct
import sys

MAGIC = b"MKLEX\0\0\0"


def pronunciation(value):
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        chosen = value.get("DEFAULT")
        if isinstance(chosen, str):
            return chosen
        for candidate in value.values():
            if isinstance(candidate, str):
                return candidate
    return None


def main():
    src, dst = sys.argv[1], sys.argv[2]
    raw = json.load(open(src))

    entries = []
    for word, value in raw.items():
        ipa = pronunciation(value)
        if not word or not ipa:
            continue
        entries.append((word.encode(), ipa.encode()))
    entries.sort(key=lambda kv: kv[0])

    blob = bytearray()
    index = []
    seen = {}
    for key, val in entries:
        # Dedupe identical strings; plenty of words share a pronunciation.
        for text in (key, val):
            if text not in seen:
                seen[text] = len(blob)
                blob += text + b"\0"
        index.append((seen[key], seen[val]))

    with open(dst, "wb") as out:
        out.write(MAGIC)
        out.write(struct.pack("<I", len(index)))
        for key_off, val_off in index:
            out.write(struct.pack("<II", key_off, val_off))
        out.write(blob)

    size = len(MAGIC) + 4 + 8 * len(index) + len(blob)
    print(f"{src} -> {dst}")
    print(f"  entries : {len(index)}")
    print(f"  bytes   : {size/1048576:.2f} MB")


if __name__ == "__main__":
    main()
