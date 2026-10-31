"""No sudden size jumps: run the shelf drag scenarios on a hidden WM and fail if the shown
width/height of the window changes by more than max(40 px, 8%) between consecutive frames.
usage: python3 jumpcheck.py <root> [app]"""
import sys, os, re, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig_lib import Rig
root=sys.argv[1]; app=sys.argv[2] if len(sys.argv)>2 else "notes"
# where each app's caption is when launched (hidden 1280x800 session)
GRAB={"notes":(555,87),"amp":(330,87),"clock":(600,87),"code":(600,100)}.get(app,(555,87))
rig=Rig(root,"jump"+app,env={"MAKEPAD_WM_TRACE_WARP":"1"}).up()
PAT=re.compile(r"warp: client (\d+) logical .*?shown LRect \{ x: ([-\d.]+), y: ([-\d.]+), w: ([\d.]+), h: ([\d.]+) \}")
def frames(since):
    out=[]
    for l in open(rig.log,errors="replace").readlines()[since:]:
        m=PAT.search(l)
        if m: out.append(tuple(float(v) for v in m.groups()[1:]))
    return out
def mark(): return sum(1 for _ in open(rig.log,errors="replace"))
def path(pts,dt,hold=0.3):
    rig.m("move",*pts[0]); time.sleep(0.4); rig.m("down",*pts[0])
    for p in pts[1:]: rig.m("move",*p); time.sleep(dt)
    time.sleep(hold); rig.m("up",*pts[-1]); time.sleep(1.5)
bad=[]
def check(name,fr):
    for a,b in zip(fr,fr[1:]):
        for i,label in ((2,"w"),(3,"h")):
            lim=max(40.0,0.08*max(a[i],b[i]))
            if abs(b[i]-a[i])>lim: bad.append((name,label,round(a[i]),round(b[i])))
    print(name,len(fr),"frames, max step",round(max([abs(b[2]-a[2]) for a,b in zip(fr,fr[1:])]+[0])))
try:
    rig.launch(app); time.sleep(3)
    m=mark(); path([(GRAB[0]+(2-GRAB[0])*j//90,GRAB[1]+(300-GRAB[1])*j//90) for j in range(91)]+[(0,300)],0.03,hold=0.9); time.sleep(0.8); check("park",frames(m))
    time.sleep(1)
    m=mark(); path([(40+j*14,300-j*2) for j in range(30)],0.05); check("pull-out",frames(m))
    m=mark(); path([(446-j*4,242+j*1) for j in range(110)]+[(0,320)],0.03,hold=0.9); check("drag back to the edge",frames(m))
finally:
    rig.down()
print("JUMPS:",bad if bad else "none")
sys.exit(1 if bad else 0)
