#!/usr/bin/env bash
# Web ARIA-mirror check in Safari through safaridriver (the Mac mini only; Safari "Allow Remote Automation" on).
# Run from the root of a tree that already has the wasm build of makepad-example-counter:
#   target/makepad-wasm-app/release/makepad-example-counter
# Serves it on 127.0.0.1:8063, opens ?a11y_selfcheck=1 in a WebDriver Safari session, reads
# window.makepadA11ySelfCheck(), presses the mirror's Increment button through the DOM (as a screen reader
# would), checks again, and stops exactly the processes it started.
set -u
dir=target/makepad-wasm-app/release/makepad-example-counter
[ -d "$dir" ] || { echo "no wasm build at $dir"; exit 2; }
python3 -m http.server 8063 --bind 127.0.0.1 --directory "$dir" > /tmp/a11y-web-http.log 2>&1 &
http=$!
safaridriver -p 4444 > /tmp/a11y-web-driver.log 2>&1 &
drv=$!
cleanup() { [ -n "${sid:-}" ] && curl -s -X DELETE "http://127.0.0.1:4444/session/$sid" >/dev/null; kill $http $drv 2>/dev/null; }
trap cleanup EXIT
sleep 2
sid=$(curl -s -X POST http://127.0.0.1:4444/session -H 'Content-Type: application/json' \
  -d '{"capabilities":{"alwaysMatch":{"browserName":"safari"}}}' | python3 -c 'import sys,json; print(json.load(sys.stdin)["value"]["sessionId"])')
echo "session $sid"
exec_js() { curl -s -X POST "http://127.0.0.1:4444/session/$sid/execute/sync" -H 'Content-Type: application/json' -d "{\"script\":$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"),\"args\":[]}" | python3 -c 'import sys,json; v=json.load(sys.stdin)["value"]; print(v if isinstance(v,str) else json.dumps(v))'; }
curl -s -X POST "http://127.0.0.1:4444/session/$sid/url" -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:8063/?a11y_selfcheck=1"}' >/dev/null
for i in $(seq 1 30); do
  r=$(exec_js 'return typeof window.makepadA11ySelfCheck === "function" ? window.makepadA11ySelfCheck() : "not ready"')
  case "$r" in *PASS*|*FAIL*) break;; esac
  sleep 1
done
echo "selfcheck: $r"
echo "mirror: $(exec_js 'return Array.from(document.querySelectorAll("[role],button,input,textarea")).filter(e=>e.closest("[data-makepad-a11y],.makepad-a11y")||true).slice(0,20).map(e=>(e.getAttribute("role")||e.tagName.toLowerCase())+":"+(e.getAttribute("aria-label")||e.textContent||"").trim().slice(0,30)).join(" | ")')"
echo "press: $(exec_js 'const b=Array.from(document.querySelectorAll("button,[role=button]")).find(e=>/increment/i.test(e.getAttribute("aria-label")||e.textContent||"")); if(!b) return "no Increment button in the mirror"; b.click(); return "clicked"')"
sleep 2
echo "after: $(exec_js 'return Array.from(document.querySelectorAll("[aria-label],[role]")).map(e=>e.getAttribute("aria-label")||"").filter(t=>/count/i.test(t)).join(" | ")')"
echo "selfcheck2: $(exec_js 'return window.makepadA11ySelfCheck()')"
