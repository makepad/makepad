#!/bin/bash
set -eu
H=$(cd "$(dirname "$0")" && pwd)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
T=$(mktemp -d)
export MAKEPAD_RUN_CACHE=off
export MAKEPAD_RUN_STD="$H/../../std"
export MAKEPAD_RUN_CACHE_DIR="$T/target"
: > "$T/actual"
for source in main.rs ok.rs; do
  python3 - "$H" "$source" "$T/units.json" <<'PY'
import json, sys
h, source, out = sys.argv[1:]
json.dump({'version':1,'roots':[0],'units':[{'pkg_id':'path+file://'+h+'#check_json@0.1.0','target':{'kind':['bin'],'name':'check_json','src_path':h+'/'+source,'edition':'2021'},'mode':'build','features':[],'dependencies':[]}]},open(out,'w'))
PY
  set +e
  # J1 includes lowering/codegen diagnostics; those are opt-in for the warm checker.
  "$BIN" check --deep --json --units "$T/units.json" check_json > "$T/raw" 2> "$T/stderr"
  status=$?
  set -e
  expected_status=1
  if [ "$source" = ok.rs ]; then expected_status=0; fi
  [ "$status" -eq "$expected_status" ] || { cat "$T/raw" "$T/stderr"; echo "unexpected status: $status (artifacts: $T)"; exit 1; }
  [ ! -s "$T/stderr" ] || { cat "$T/stderr"; echo "unexpected human output in JSON mode"; exit 1; }
  python3 - "$H" "$T/raw" "$T/actual" <<'PY'
import json,sys
h,source,out=sys.argv[1:]
rows=[json.loads(line) for line in open(source)]
assert rows[-1]['type']=='summary'
assert isinstance(rows[-1]['ms'],int) and rows[-1]['ms']>=0
assert rows[-1]['errors']==len(rows)-1
rows[-1]['ms']=0
for d in rows[:-1]:
    d['file']=d['file'].removeprefix(h+'/')
    if d['suggestion'] and d['suggestion']['replacement']:
        d['suggestion']['replacement'][0]=d['suggestion']['replacement'][0].removeprefix(h+'/')
with open(out,'a') as f:
    for d in rows: f.write(json.dumps(d,ensure_ascii=False,separators=(',',':'))+'\n')
PY
done
cat > "$T/expected" <<'EXPECTED'
{"severity":"error","code":"RR0001","message":"unresolved import `crate::Widgte`","file":"main.rs","line":3,"col":12,"end_line":3,"end_col":18,"phase":"resolve","notes":[],"suggestion":{"message":"did you mean `Widget`?","replacement":["main.rs",3,12,3,18,"Widget"]}}
{"severity":"error","code":"E0412","message":"cannot find type `Widgte` in this scope","file":"main.rs","line":4,"col":11,"end_line":4,"end_col":17,"phase":"typeck","notes":[],"suggestion":{"message":"did you mean `Widget`?","replacement":["main.rs",4,11,4,17,"Widget"]}}
{"severity":"error","code":"RT0006","message":"signature has unresolved types","file":"main.rs","line":4,"col":1,"end_line":4,"end_col":3,"phase":"typeck","notes":["fix the signature errors first"],"suggestion":null}
{"severity":"error","code":"E0412","message":"cannot find type `Missing` in this scope","file":"main.rs","line":6,"col":1,"end_line":6,"end_col":7,"phase":"typeck","notes":[],"suggestion":null}
{"severity":"error","code":"RT0006","message":"signature has unresolved types","file":"main.rs","line":6,"col":1,"end_line":6,"end_col":7,"phase":"typeck","notes":["fix the signature errors first"],"suggestion":null}
{"severity":"error","code":"RM0001","message":"cannot find macro `greetign` in this scope","file":"main.rs","line":9,"col":1,"end_line":9,"end_col":9,"phase":"macro","notes":[],"suggestion":{"message":"did you mean `greeting`?","replacement":["main.rs",9,1,9,9,"greeting"]}}
{"type":"summary","errors":6,"warnings":0,"ms":0}
{"type":"summary","errors":0,"warnings":0,"ms":0}
EXPECTED
if ! diff -u "$T/expected" "$T/actual"; then
  cat "$T/stderr"
  echo "check_json: FAILED (artifacts: $T)"
  exit 1
fi
echo "check_json: ok"
