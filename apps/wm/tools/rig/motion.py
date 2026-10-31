"""Record shelf-motion scenarios headless at TRUE frame spacing (virtual clock: one pointer script
step and one rendered frame per 1/60 s), with a per-frame readback, an MP4, a contact sheet, a
per-frame CSV (from the WM's MAKEPAD_WM_MOTION trace) and a plot, and judge smoothness.
usage: python3 motion.py <root> <outdir> [scenario ...]
scenarios: hidden visible click_out press_out resize"""
import sys, os, re, math, time, tempfile, subprocess
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rig_lib import Rig
from PIL import Image, ImageDraw
root=sys.argv[1]; outdir=sys.argv[2]; names=sys.argv[3:] or ["hidden","visible","click_out","press_out","resize"]
os.makedirs(outdir,exist_ok=True)
MOT=re.compile(r"MOTION ([\d.]+) (\d+) ([-\d.]+) ([-\d.]+) ([\d.]+) ([\d.]+) (\w+)")
def ease(t): return t*t*(3-2*t)

def run(name):
    state=tempfile.mkdtemp(prefix="motion-")
    if name=="hidden":
        os.makedirs(state+"/wm",exist_ok=True); open(state+"/wm/current-shelf-autohide","w").write("left")
    orig=tempfile.mkdtemp; tempfile.mkdtemp=lambda **k: state
    rig=Rig(root,"motion"+name,env={"MAKEPAD_WM_MOTION":"1"},virtual=True)
    try: rig.up()
    finally: tempfile.mkdtemp=orig
    frames=[]            # per frame: (png path)
    marks=[]
    fdir=f"/tmp/motion-{name}-frames"; os.makedirs(fdir,exist_ok=True)
    for f in os.listdir(fdir): os.remove(f"{fdir}/{f}")   # literal names listed from our own dir
    def step(n=1): rig.r("step",f"frames={n}","fps=60")
    def logn(): return sum(1 for _ in open(rig.log,errors="replace"))
    recording=[False]
    def frame(events):
        """send the pointer events of this frame, render one frame, read it back"""
        for e in events: rig.m(*e)
        m=logn(); step(1)
        if recording[0]:
            p=f"{fdir}/f{len(frames):04d}.png"; rig.r("grab",f"out={p}","scale=0.5")
            frames.append(p); marks.append((m,logn()))
    def path_events(pts,nframes):
        segs=list(zip(pts,pts[1:])); lens=[math.hypot(b[0]-a[0],b[1]-a[1]) for a,b in segs]; L=sum(lens) or 1
        def at(t):
            d=ease(t)*L; i=0
            while i<len(lens)-1 and d>lens[i]: d-=lens[i]; i+=1
            a,b=segs[i]; f=d/(lens[i] or 1); return (int(a[0]+(b[0]-a[0])*f),int(a[1]+(b[1]-a[1])*f))
        return [[("move","k=move")+()] for _ in []] or [at((k+1)/nframes) for k in range(nframes)]
    def mv(p): return ("mouse","k=move",f"x={p[0]}",f"y={p[1]}")
    def down(p): return ("mouse","k=down",f"x={p[0]}",f"y={p[1]}")
    def up(p): return ("mouse","k=up",f"x={p[0]}",f"y={p[1]}")
    def mrun(*a):
        return rig.r(*a)
    # the rig's m() helper only takes (k,x,y); use raw r() for events
    def ev(e): rig.r(*e)
    def frame_ev(evs):
        for e in evs: ev(e)
        m=logn(); step(1)
        if recording[0]:
            p=f"{fdir}/f{len(frames):04d}.png"; rig.r("grab",f"out={p}","scale=0.5")
            frames.append(p); marks.append((m,logn()))
    def drag(pts,dur_frames,press=True,release=True,hold=0):
        pp=path_events(pts,dur_frames)
        if press: frame_ev([mv(pts[0]),down(pts[0])])
        for k,p in enumerate(pp):
            # two moves per frame: the midpoint (about 8 ms apart) then the point
            prev=pp[k-1] if k else pts[0]
            frame_ev([mv(((prev[0]+p[0])//2,(prev[1]+p[1])//2)),mv(p)])
        for _ in range(hold): frame_ev([mv(pts[-1])])
        if release: frame_ev([up(pts[-1])])
    def settle(n): 
        for _ in range(n): frame_ev([])
    def last_rect(client):
        rect=None
        for l in open(rig.log,errors="replace").readlines()[::-1][:400]:
            m=MOT.search(l)
            if m and int(m.group(2))==client: return tuple(float(v) for v in m.groups()[2:6])
    try:
        rig.launch("notes"); step(150)
        tracked=2
        park=lambda: (drag([(555,87),(300,200),(1,300)],40,hold=20),)
        if name in ("click_out","press_out"):
            recording[0]=False; drag([(555,87),(300,200),(1,300)],40,hold=30); settle(90)
        if name=="hidden":
            recording[0]=True
            rig.r("cap","start",f"path={outdir}/{name}.mp4","fps=60","overwrite=1")
            settle(10)
            drag([(555,87),(300,160),(60,260),(0,300)],55,hold=40)    # to the edge, it opens, drop
            settle(80)
        elif name=="visible":
            recording[0]=True
            rig.r("cap","start",f"path={outdir}/{name}.mp4","fps=60","overwrite=1")
            settle(10)
            drag([(555,87),(300,160),(120,260),(40,300)],55,hold=40)
            settle(80)
        elif name=="click_out":
            recording[0]=True
            rig.r("cap","start",f"path={outdir}/{name}.mp4","fps=60","overwrite=1")
            settle(10)
            tile=last_rect(tracked) or (40,295,150,130)
            cx,cy=int(tile[0]+tile[2]/2),int(tile[1]+tile[3]/2)
            frame_ev([mv((cx,cy)),down((cx,cy))]); frame_ev([up((cx,cy))])     # click: unfold
            settle(50)
            half=last_rect(tracked)
            hx,hy=int(half[0]+half[2]/2),int(half[1]+4)                       # the slim caption strip
            drag([(hx,hy),(300,200),(560,160)],45)
            settle(90)
        elif name=="press_out":
            recording[0]=True
            rig.r("cap","start",f"path={outdir}/{name}.mp4","fps=60","overwrite=1")
            settle(10)
            tile=last_rect(tracked) or (40,295,150,130)
            cx,cy=int(tile[0]+tile[2]/2),int(tile[1]+tile[3]/2)
            drag([(cx,cy),(300,300),(600,250)],45)
            settle(90)
        elif name=="resize":
            recording[0]=True
            rig.r("cap","start",f"path={outdir}/{name}.mp4","fps=60","overwrite=1")
            settle(10)
            r=last_rect(tracked); rx,ry=int(r[0]+r[2]),int(r[1]+r[3]/2)
            drag([(rx,ry),(1100,ry),(1279,ry)],50,hold=30)
            settle(60)
        cap=rig.r("cap","stop")
        lines=open(rig.log,errors="replace").readlines()
        rows=[]
        for i,(a,b) in enumerate(marks):
            seg=[MOT.search(l) for l in lines[a:b]]
            seg=[m for m in seg if m and int(m.group(2))==tracked]
            if seg:
                g=seg[-1].groups(); rows.append((i,i/60.0,float(g[2]),float(g[3]),float(g[4]),float(g[5]),g[6]))
            else: rows.append((i,i/60.0,None,None,None,None,"-"))
        return rows,frames,cap
    finally:
        rig.down()

def analyse(name,rows):
    bad=[]
    prev=None
    for r in rows:
        if r[2] is None: continue
        if prev is not None:
            for k,lab in ((4,"w"),(5,"h")):
                lim=max(40.0,0.08*max(r[k],prev[k]))
                if abs(r[k]-prev[k])>lim: bad.append((r[0],lab,round(prev[k]),round(r[k]),prev[6],r[6]))
            if r[6]!=prev[6] and (abs(r[4]-prev[4])>6 or abs(r[5]-prev[5])>6): bad.append((r[0],"source",prev[6],r[6],round(prev[4]),round(r[4])))
        prev=r
    return bad

def plot(name,rows,path):
    W,H=1200,600; im=Image.new("RGB",(W,H),(18,18,24)); d=ImageDraw.Draw(im)
    rs=[r for r in rows if r[2] is not None]
    if not rs: im.save(path); return
    T=max(r[1] for r in rs) or 1
    series=[("w",4,(90,200,255),0,700),("h",5,(255,180,80),0,650),("x",2,(120,255,140),-200,1000),("y",3,(255,110,150),-100,800)]
    for lab,k,col,lo,hi in series:
        pts=[(60+(r[1]/T)*(W-100),H-40-((r[k]-lo)/(hi-lo))*(H-80)) for r in rs]
        d.line(pts,fill=col,width=2); d.text((W-90,20+series.index((lab,k,col,lo,hi))*14),lab,fill=col)
    d.text((10,6),f"{name}: shown rect of the tracked window per frame (w/h/x/y, 60 fps)",fill=(220,220,230))
    last=None
    for r in rs:
        if r[6]!=last: d.line([(60+(r[1]/T)*(W-100),30),(60+(r[1]/T)*(W-100),H-30)],fill=(70,70,90)); d.text((62+(r[1]/T)*(W-100),H-30),r[6],fill=(160,160,180))
        last=r[6]
    im.save(path)

def sheet(frames,path):
    sel=frames[::2][:60]
    if not sel: return
    ims=[Image.open(f).convert("RGB").resize((320,200)) for f in sel]
    cols=6; rws=(len(ims)+cols-1)//cols
    S=Image.new("RGB",(cols*320,rws*200))
    for i,im in enumerate(ims): S.paste(im,((i%cols)*320,(i//cols)*200))
    S.save(path)

total=0
for name in names:
    rows,frames,cap=run(name)
    with open(f"{outdir}/{name}.csv","w") as f:
        f.write("frame,t,x,y,w,h,source\n")
        for r in rows: f.write(",".join("" if v is None else (f"{v:.1f}" if isinstance(v,float) else str(v)) for v in r)+"\n")
    plot(name,rows,f"{outdir}/{name}.plot.png"); sheet(frames,f"{outdir}/{name}.sheet.png")
    bad=analyse(name,rows); total+=len(bad)
    print(name,len(rows),"frames;",("SMOOTH" if not bad else f"{len(bad)} JUMPS: {bad[:6]}"))
sys.exit(1 if total else 0)
