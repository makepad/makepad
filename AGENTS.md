# Studio Bridge Runbook

## Assumptions
- Studio is started manually by the user.
- Bridge target is `ip:port` only (no `http://`, no `ws://`), normally `127.0.0.1:8001`.
  - Use `127.0.0.1:8002` only if Studio reports fallback because `8001` is occupied.
- Keep one persistent bridge process for the whole interaction.

## Start Bridge
- Command:
  - `cargo run -p cargo-makepad --release -- studio --studio=127.0.0.1:8001`
- Send newline-delimited JSON requests on stdin.
- Read newline-delimited JSON responses on stdout.

## Request Protocol (JSON Lines)
- `{"ListBuilds":[]}`
- `{"CargoRun":{"args":["-p","makepad-example-todo","--release"],"root":null,"startup_query":"id:todo_input"}}`
- `{"Stop":{"build_id":38160721318170}}`
- `{"WidgetTreeDump":{"build_id":38160721318170}}`
- `{"WidgetQuery":{"build_id":38160721318170,"query":"id:todo_input"}}`
- `{"Screenshot":{"build_id":38160721318170,"kind_id":0}}`
- `{"Click":{"build_id":38160721318170,"x":1274,"y":342,"button":1,"auto_dump":false}}`
- `{"TypeText":{"build_id":38160721318170,"text":"hello","replace_last":false,"was_paste":false,"auto_dump":false}}`
- `{"Return":{"build_id":38160721318170,"auto_dump":false}}`

## Recommended Control Flow
1. Start bridge process once.
2. `CargoRun` and wait for `Started`.
3. Use `WidgetQuery` / `WidgetTreeDump` to get click targets.
4. For text input, click field first, then send text, then return.
5. Keep control packets compact (`auto_dump:false` on click/type/return for low latency).

## One-Flow Input Burst
- Send this as one stdin write (multiple JSON lines, no sleeps):
  - `Click` (input field center)
  - `TypeText`
  - `Return`
- Then request `WidgetTreeDump` or `Screenshot` to confirm.

## Coordinates
- Use coordinates from dump as-is.
- `W3` dump uses integer pixel coordinates in the same space expected by `Click`.
- Do not apply extra DPI math in the agent loop.

## Reliability Notes
- `Screenshot` can arrive before visible redraw after rapid input bursts.
  - If screenshot looks stale, request a follow-up `WidgetTreeDump`/`Screenshot`.
- If input does nothing:
  - Verify `build_id` with `ListBuilds`.
  - Refresh dump and retry click on input before typing.
- If request errors with no active websocket:
  - app is not connected yet; wait for startup completion and retry.
