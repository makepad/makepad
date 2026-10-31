import subprocess as sp, time, os, json, sys
HOME=os.path.expanduser("~")
class Rig:
    def __init__(self, root, tag, env=None, virtual=False, trace="gpu.pass"):
        self.root=root; self.tag=tag; self.env=env or {}; self.virtual=virtual; self.trace=trace
        self.state=None; self.log=f"/tmp/rig-{tag}-app.log"
        self.R=f"{root}/target/release/makepad-remote"
    def up(self):
        uid=os.getuid(); xdg=f"/run/user/{uid}"
        self.sock=f"wayland-rig-{tag_safe(self.tag)}-{os.getpid()}"
        e=dict(os.environ, XDG_RUNTIME_DIR=xdg)
        self.wp=sp.Popen(["nice","weston","--backend=headless","--renderer=gl",f"--socket={self.sock}","--width=1480","--height=1000","--idle-time=0"],stdout=open(f"/tmp/rig-{self.tag}-weston.log","w"),stderr=sp.STDOUT,env=e)
        time.sleep(4)
        import tempfile; self.state=tempfile.mkdtemp(prefix=f"rig-{tag_safe(self.tag)}-")
        e.update(WAYLAND_DISPLAY=self.sock,SANDBOX_MUTE="1",MAKEPAD_HIDE_WINDOWS="1",MAKEPAD_WM_BACKDROP="warp-grid",MAKEPAD_WM_NO_WARM="1",
                 MAKEPAD_WM_ROOT=self.root,MAKEPAD_HOME=self.state,XDG_CONFIG_HOME=self.state+"/c",XDG_DATA_HOME=self.state+"/d",XDG_CACHE_HOME=self.state+"/k")
        if self.trace: e["MAKEPAD_TRACE"]=self.trace
        e.update(self.env)
        args=[f"{self.root}/target/release/makepad-app-wm","--remote","--window=1280x800@2","-scale=1.3"]
        if self.virtual: args.append("--virtual-clock")
        self.ap=sp.Popen(["nice"]+args,cwd=self.root,stdout=open(self.log,"w"),stderr=sp.STDOUT,env=e,start_new_session=True)
        time.sleep(8)
        return self
    def down(self):
        try: self.ap.terminate(); self.ap.wait(10)
        except Exception: pass
        try: self.wp.terminate(); self.wp.wait(10)
        except Exception: pass
    def r(self,*a):
        return sp.run([self.R,"app",str(self.ap.pid),*a],capture_output=True,text=True).stdout.strip()
    def tool(self,name,args="{}"):
        """One tool of the WM's `app` service (wm.motion_trace, os.launch): its result text."""
        for l in self.r("call",name,args).splitlines():
            try: d=json.loads(l)
            except ValueError: continue
            if "Result" in d: return d["Result"][0]["text"]
        return ""
    def m(self,k,x,y): return self.r("mouse",f"k={k}",f"x={x}",f"y={y}")
    def ff(self,app,t=240):
        for _ in range(t):
            if any((app+" client" in l and "first frame" in l) for l in open(self.log,errors="replace")): return True
            time.sleep(1)
        return False
    def launch(self,app): 
        self.tool("os.launch",json.dumps({"app":app})); return self.ff(app)
def tag_safe(t): return "".join(c for c in t if c.isalnum())
