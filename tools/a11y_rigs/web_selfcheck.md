# C13 web mirror check (lane lead, Mac mini only)

Do not build or open this app on the user's Mac. Run the following on the mini through
the lane's `minibuild.sh` wrapper (from the edited worktree), which serializes mini jobs:

```sh
/Users/admin/makepad/local/agent_state/a11y/minibuild.sh "$PWD" cargo build --release --offline -j6 -p cargo-makepad
/Users/admin/makepad/local/agent_state/a11y/minibuild.sh "$PWD" ./target/release/cargo-makepad wasm --no-threads build --release --offline -j6 -p makepad-example-counter --no-default-features
```

This uses the existing nightly toolchain and rust-src; install nothing if either is missing.
Node 203 can check/build the standard wasm target, but cannot run the repository's web
packaging command because it has no `rustup` executable. The lane lead must complete the
packaging/browser gate on an equipped machine. `--no-threads` lets the static server below
serve the example without cross-origin isolation headers.

In a shell **on the mini**, after the wrapper finishes:

```sh
cd /Users/dev/lanes/a11y/a11y-c13
python3 -m http.server 8063 --bind 127.0.0.1 --directory target/makepad-wasm-app/release/makepad-example-counter
```

Keep that server in the foreground; Ctrl-C stops this exact owned process. In the mini's
browser, open `http://127.0.0.1:8063/?a11y_selfcheck=1`. Do not open it on the user's Mac.
The first nonempty tree logs `makepad a11y self-check: PASS (N nodes)` or `FAIL` with node
IDs and mismatched properties in the browser's developer console. Later, evaluate
`window.makepadA11ySelfCheck()` to repeat the comparison after each interaction.
There is no web `ui oscheck` remote endpoint in this checkout.

Check Tab/Shift-Tab through the Increment button, Enter, Space and a screen-reader click.
Each activation must change the counter once. Resize the page, scroll its container if
embedded, and run the self-check again. Check the native browser accessibility inspector
for names, roles and bounds. This is the browser/accessibility proof; the Node logic rig
does not verify rendered bounds or the browser accessibility tree.

The default is on. Check `window.makepadA11ySetEnabled(false)`, then `true`: the mirror
disappears, the builder stops, then a fresh full tree appears. Reload with `?a11y=0` to
check startup opt-out. A page can also set `window.makepadA11yEnabled = false` before app
startup. Turning the mirror off restores the existing IME textarea; turning it on removes
that textarea and uses the semantic input itself as `app.text_area`.

For automation, listen before activating a control:

```js
window.addEventListener("makepad-a11y-action-done", event => console.log(event.detail));
```

The result is `{req: BigInt, result: number}` with the Rust `ActionResult` discriminant
(0 handled, 1 fallback, 2 missing, 3 disabled, 4 unsupported). The adapter retains no
completion history.

After C2's widget action handlers are integrated, repeat with `makepad-example-text-input`
and `makepad-example-uizoo` in the build/server commands. Edit accented and astral text,
backspace, paste, cut, select backwards, use IME composition, and read/change a slider.
Confirm the widget value and caret change through semantic actions and that only the
focused semantic input owns IME focus. This lane's base has only FOCUS on TextInput, so
editing and range action coverage is pending C2, not a passing end-to-end check.

Record the startup/self-check output, completion results and browser checks in the local
C13 report. Close the owned browser tab and stop the server when finished. Remove the
mini lane directory only after saving evidence and checking no process still uses it.

Node-only checks, on node 203 through `nodebuild.sh`:

```sh
cargo test --release --offline -j8 -p makepad-a11y
node tools/a11y_rigs/web_logic.mjs
```

The Rust test emits its startup-generated codec and actual wire bytes to installed Node,
checks all fields and notifications, and decodes all seven action argument forms back in
Rust. The DOM double checks diff identity, hierarchy, focus, action routing, Unicode
selection/composition and enable/disable without a browser or GPU.
