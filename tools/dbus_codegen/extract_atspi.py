# Runs on a Linux node: prints every AT-SPI interface's introspection XML embedded in the installed
# accessibility bridge library, separated by NUL lines with the interface name.
import re, sys, hashlib
path = sys.argv[1]
data = open(path, 'rb').read()
sys.stderr.write("sha256 %s %s\n" % (hashlib.sha256(data).hexdigest(), path))
seen = set()
for m in re.finditer(rb'<interface name="(org\.a11y\.atspi[A-Za-z.]*)".*?</interface>', data, re.S):
    name = m.group(1).decode()
    if name in seen: continue
    seen.add(name)
    sys.stdout.write("@@@ " + name + "\n")
    sys.stdout.write(m.group(0).decode() + "\n")
