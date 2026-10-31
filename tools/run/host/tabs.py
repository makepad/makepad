# tabs.py <makepad> <pid>: click every tab_* button of a running app once (first visits), through
# `makepad inspect`. Used by train.sh for Stage's tab burst.
import re, subprocess, sys, time
BIN, PID = sys.argv[1], sys.argv[2]
def ins(*a, timeout=30):
    try: return subprocess.run([BIN, 'inspect', PID] + list(a), capture_output=True, text=True, timeout=timeout).stdout
    except Exception as e: return 'ERR %s' % e
found = {}
for _ in range(40):
    for line in ins('dump', 'paths=1').splitlines():
        m = re.search(r'\b(tab_[A-Za-z0-9_]+)\b\S*\s+\S+\s+(-?[\d.]+)\s+(-?[\d.]+)\s+([\d.]+)\s+([\d.]+)', line)
        if m and m.group(1) not in found and float(m.group(4)) > 0 and float(m.group(5)) > 0:
            found[m.group(1)] = tuple(float(m.group(i)) for i in range(2, 6))
    if found: break
    time.sleep(0.25)
order = list(found)
for name in order[1:] + order[:1]:
    x, y, w, h = found[name]
    t = time.time()
    ins('click', 'x=%d' % (x + w / 2), 'y=%d' % (y + h / 2), 'wait=1')
    print('tab %s %.0f ms' % (name, (time.time() - t) * 1e3), flush=True)
    time.sleep(0.3)
print('tabs visited: %d' % len(order), flush=True)
