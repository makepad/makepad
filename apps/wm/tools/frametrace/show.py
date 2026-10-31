#!/usr/bin/env python3
"""Print a frame range of a frametrace compactly.

    show.py frames.jsonl FROM TO [--app notes] [--cap]
"""
import argparse, json


def f(r):
    return "[" + ",".join(f"{v:.0f}" for v in r) + "]" if r else "-"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace")
    ap.add_argument("lo", type=int)
    ap.add_argument("hi", type=int)
    ap.add_argument("--app", default="")
    ap.add_argument("--cap", action="store_true", help="also windows drawn into captures")
    a = ap.parse_args()
    for line in open(a.trace):
        fr = json.loads(line)
        if fr["n"] < a.lo:
            continue
        if fr["n"] > a.hi:
            break
        d = fr["disp"][0]
        sl = d["slide"]
        head = f'#{fr["n"]} t={fr["t"]:.3f} cur={d["cur"]} act={d["active"]} hide=[{d["hide"][0]:.2f},{d["hide"][1]:.2f}]'
        if sl:
            head += f' slide={sl["pos"]:.3f}({sl["from"]}->{sl["to"]})'
        if fr["made"]:
            head += " MADE " + " ".join(f'{m["key"]}:{m["m"]}' for m in fr["made"])
        if fr["shots"]:
            head += " SHOTS " + " ".join(f'{s["key"]}@{s["off"]:.2f}{"" if s["drawn"] else "!"}' for s in fr["shots"])
        for k in ("leave", "enter", "unfold"):
            if fr[k]:
                head += f" {k}={json.dumps(fr[k])}"
        if fr["dragging"]:
            head += f' drag={fr["dragging"]}'
        if fr["ev"]:
            head += f' ev={fr["ev"]}'
        print(head)
        for w in fr["w"]:
            if w["cap"] is not None and not a.cap:
                continue
            if a.app and w.get("app") != a.app:
                continue
            an = w.get("anim") or {}
            h = w.get("host") or {}
            p = w.get("pose") or {}
            print(f'   {"CAP"+str(w["cap"]) if w["cap"] else "   "} {w["c"]}:{w.get("app",""):<10} {w["src"]:<8} s={f(w["s"])} d={f(w["d"])} '
                  f'tgt={f(an.get("tgt"))}{" mv" if an.get("mv") else ""}{" open%.2f" % an["open"] if an.get("open", 1) < 1 else ""} '
                  f'req={f(w.get("req"))} cfg={f(h.get("cfg"))} px={h.get("px")} ser={h.get("ser")} '
                  f'pose(side={p.get("side")},k={p.get("k", 1):.2f}{",cmp" if p.get("cmp") else ""}) '
                  f'{"half=" + f(w["half"]) if w.get("half") else ""}{" swl=" + f(w["swl"]) if w.get("swl") else ""}{" FRZ" if w.get("frz") else ""}{" HID" if w.get("hid") else ""}{" DRAG" if w.get("drag") else ""}')


if __name__ == "__main__":
    main()
