#!/usr/bin/env python3
"""Pack the Mandarin front end's data from three MIT-licensed text sources.

    pinyin-data          https://github.com/mozillazg/pinyin-data          pinyin.txt
    phrase-pinyin-data   https://github.com/mozillazg/phrase-pinyin-data   large_pinyin.txt
    jieba                https://github.com/fxsjy/jieba                    jieba/dict.txt

    python3 build_zh_lexicon.py pinyin.txt large_pinyin.txt dict.txt <out_dir>

The outputs are hosted with the Kokoro model (src/g2p/data.rs names their URL,
size and sha256; update those after a rebuild), not kept in git.

Writes two MKLEX files (the format `build_lexicon.py` documents; `g2p::packed`
reads them), entries sorted by key bytes:

    zh_pinyin.bin   key: a character (its usual reading) or a polyphonic word /
                    phrase; value: space-separated syllables, tone digit last,
                    ü written v ("lv4"), neutral tone 5. Phrases are kept only
                    when their reading differs from their characters' usual ones.
    zh_words.bin    key: a word the segmenter knows; value: "<freq> <pos>".
                    The key "\\0total" holds the sum of all frequencies (also of
                    the words pruned for size), so word probabilities match the
                    unpruned dictionary.
"""

import re
import struct
import sys
import unicodedata

MAGIC = b"MKLEX\0\0\0"
MIN_FREQ = 10         # drops the long tail of rare words; their frequency stays in the total
MAX_WORD_CHARS = 12

# pinyin-data writes tone marks; map each vowel+mark to (base vowel, tone).
MARKS = {
    "ā": ("a", 1), "á": ("a", 2), "ǎ": ("a", 3), "à": ("a", 4),
    "ē": ("e", 1), "é": ("e", 2), "ě": ("e", 3), "è": ("e", 4),
    "ī": ("i", 1), "í": ("i", 2), "ǐ": ("i", 3), "ì": ("i", 4),
    "ō": ("o", 1), "ó": ("o", 2), "ǒ": ("o", 3), "ò": ("o", 4),
    "ū": ("u", 1), "ú": ("u", 2), "ǔ": ("u", 3), "ù": ("u", 4),
    "ǖ": ("v", 1), "ǘ": ("v", 2), "ǚ": ("v", 3), "ǜ": ("v", 4),
    "ń": ("n", 2), "ň": ("n", 3), "ǹ": ("n", 4), "ḿ": ("m", 2),
    "ế": ("ê", 2), "ề": ("ê", 4), "ê̄": ("ê", 1), "ê̌": ("ê", 3),
}


def numeric(syllable):
    """`hàng` -> `hang4`; no mark -> tone 5; ü -> v."""
    tone = 5
    out = []
    syllable = unicodedata.normalize("NFC", syllable)
    for ch in syllable:
        if ch in MARKS:
            ch, tone = MARKS[ch]
        elif ch == "ü":
            ch = "v"
        out.append(ch)
    return "".join(out) + str(tone)


def is_han(text):
    return all("一" <= c <= "鿿" for c in text)


def main():
    pinyin_txt, phrase_txt, dict_txt, out_dir = sys.argv[1:5]

    # Usual reading of each character: the first one pinyin-data lists.
    usual = {}
    for line in open(pinyin_txt, encoding="utf8"):
        if line.startswith("#") or ":" not in line:
            continue
        code, rest = line.split(":", 1)
        readings = rest.split("#")[0].strip().split(",")
        char = chr(int(code.strip()[2:], 16))
        if "一" <= char <= "鿿" and readings and readings[0].strip():
            usual[char] = numeric(readings[0].strip())

    # The front end (misaki's) makes the particle 地 neutral by default.
    usual["地"] = "de5"

    pinyin = dict(usual)
    kept = 0
    for line in open(phrase_txt, encoding="utf8"):
        if line.startswith("#") or ":" not in line:
            continue
        word, rest = line.split(":", 1)
        word = word.strip()
        syllables = [numeric(s) for s in rest.split("#")[0].split()]
        if len(word) < 2 or len(word) != len(syllables) or not is_han(word):
            continue
        if all(usual.get(c) == s for c, s in zip(word, syllables)):
            continue  # nothing a phrase entry would change
        pinyin[word] = " ".join(syllables)
        kept += 1

    # The few phrase overrides the Kokoro front end loads on top.
    for word, readings in {
        "开户行": "kai1 hu4 hang2", "发卡行": "fa4 ka3 hang2", "放款行": "fang4 kuan3 hang2",
        "茧行": "jian3 hang2", "行号": "hang2 hao4", "各地": "ge4 di4", "借还款": "jie4 huan2 kuan3",
        "时间为": "shi2 jian1 wei2", "为准": "wei2 zhun3", "色差": "se4 cha1", "嗲": "dia3",
        "呗": "bei5", "不": "bu4", "咗": "zuo5", "嘞": "lei5", "掺和": "chan1 huo5",
    }.items():
        pinyin[word] = readings

    words = {}
    total = 0
    for line in open(dict_txt, encoding="utf8"):
        parts = line.split()
        if len(parts) < 2 or not parts[1].isdigit():
            continue
        word, freq = parts[0], int(parts[1])
        pos = parts[2] if len(parts) > 2 else "x"
        total += freq
        if freq < MIN_FREQ or len(word) > MAX_WORD_CHARS:
            continue
        words[word] = (freq, pos)

    write(out_dir + "/zh_pinyin.bin", sorted((k.encode(), v.encode()) for k, v in pinyin.items()))
    entries = [(k.encode(), f"{f} {p}".encode()) for k, (f, p) in words.items()]
    entries.append((b"\0total", str(total).encode()))
    write(out_dir + "/zh_words.bin", sorted(entries))
    print(f"pinyin: {len(pinyin)} entries ({len(usual)} characters, {kept} phrases)")
    print(f"words : {len(words)} entries, total frequency {total}")


def write(path, entries):
    blob = bytearray()
    seen = {}
    index = []
    for key, val in entries:
        for text in (key, val):
            if text not in seen:
                seen[text] = len(blob)
                blob += text + b"\0"
        index.append((seen[key], seen[val]))
    with open(path, "wb") as out:
        out.write(MAGIC)
        out.write(struct.pack("<I", len(index)))
        for k, v in index:
            out.write(struct.pack("<II", k, v))
        out.write(blob)
    print(f"{path}: {(len(MAGIC) + 4 + 8 * len(index) + len(blob)) / 1048576:.2f} MB")


if __name__ == "__main__":
    main()
