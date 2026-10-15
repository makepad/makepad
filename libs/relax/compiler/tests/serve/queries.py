#!/usr/bin/env python3
"""Exercise real query CLI fallbacks without a listener (for restricted build sandboxes)."""
import json, os, pathlib, shutil, subprocess, sys, tempfile, time
relax = pathlib.Path(sys.argv[1]).resolve()
here = pathlib.Path(__file__).resolve().parent
work = pathlib.Path(tempfile.mkdtemp(prefix='relax-queries-'))
fixture = work/'fixture'
shutil.copytree(here/'fixture',fixture)
source = fixture/'main.rs'
source.write_text(source.read_text().replace('Missing','i32'))
env = dict(os.environ,MAKEPAD_RUN_CACHE_DIR=str(work/'cache'),MAKEPAD_RUN_CACHE='off',MAKEPAD_RUN_STD=str(here.parents[1]/'std'),
           CARGO_NET_OFFLINE='true',MAKEPAD_RUN_OFFLINE='1')
def query(method,*args,pkg='serve_fixture'):
    start=time.monotonic()
    result=subprocess.run([str(relax),method,pkg,*map(str,args)],cwd=fixture,env=env,capture_output=True,text=True,timeout=180)
    assert result.returncode==0,(result.returncode,result.stderr,result.stdout,work)
    value=json.loads(result.stdout)
    print(method, f'{(time.monotonic()-start)*1000:.1f} ms',json.dumps(value),flush=True)
    return value
schema=query('schema','crate::Counter')
assert not any(t.endswith('::Pattern') for t in schema['traits']),schema
assert any(m['name']=='borrow' and '&Counter' in m['signature'] for m in schema['methods']),schema
assert schema['kind']=='struct' and 'documented counter' in schema['docs'] and 'Used by' in schema['docs'],schema
assert schema['fields'][0]['type']=='i32' and 'Current count' in schema['fields'][0]['docs'],schema
assert any(m['name']=='read' and 'Read the count' in m['docs'] for m in schema['methods']),schema
for line,col,kind,ty in [(16,17,'local','Counter'),(16,23,'field','i32'),(17,22,'fn','i32'),(10,36,'local','Counter')]:
    v=query('hover',source,line,col)
    assert v and v['kind']==kind and ty in v['type'],v
v=query('definition',source,18,19)
assert v and v[0]['line']==13,v
other=query('schema','Other',pkg='serve_other')
assert other['traits'] and any(m['name']=='read' for m in other['methods']),other
choice=query('schema','Choice',pkg='serve_other')
assert [v['name'] for v in choice['variants']]==['Empty','Value','Named'],choice
assert choice['variants'][1]['fields'][0]['name']=='0',choice
print('queries: PASS; artifacts',work)
# Exercise the shared J2 session over its existing watch pipe when TCP is unavailable.
# This is supplementary coverage, not a replacement for run.sh's transport assertions.
import queue, threading
errors=open(work/'watch.stderr','w')
watch=subprocess.Popen([str(relax),'check','--watch','--json','serve_fixture'],cwd=fixture,env=env,
                       stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,text=True,bufsize=1)
output=queue.Queue()
def collect():
    for line in watch.stdout: output.put(line)
    output.put(None)
threading.Thread(target=collect,daemon=True).start()
def batch(label):
    diagnostics=[]
    while True:
        line=output.get(timeout=180)
        assert line is not None,(watch.poll(),(work/'watch.stderr').read_text())
        row=json.loads(line)
        if row.get('type')=='summary':
            print(label,json.dumps(row),flush=True)
            return diagnostics,row
        diagnostics.append(row)
try:
    diagnostics,summary=batch('warm initial')
    assert not diagnostics
    bad=source.read_text().replace('fn broken(_: i32)', 'fn broken(_: Missing)')
    watch.stdin.write(json.dumps(dict(method='didChange',path=str(source),text=bad))+'\n'); watch.stdin.flush()
    diagnostics,summary=batch('warm signature error')
    assert any(d['code']=='E0412' for d in diagnostics),diagnostics
    watch.stdin.write(json.dumps(dict(method='didChange',path=str(source),text=source.read_text()))+'\n'); watch.stdin.flush()
    diagnostics,summary=batch('warm fixed')
    assert not diagnostics,diagnostics
    watch.stdin.write('{"method":"shutdown"}\n'); watch.stdin.flush()
    assert watch.wait(timeout=15)==0
finally:
    if watch.poll() is None:
        watch.terminate()
        try: watch.wait(timeout=15)
        except subprocess.TimeoutExpired: watch.kill(); watch.wait()
    errors.close()
print('warm session: PASS')
