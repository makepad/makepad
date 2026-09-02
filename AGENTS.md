# Makepad Agent Runbook

> **Driving a running app: use `--remote`.** Every makepad app started with
> `--remote` serves a tiny localhost HTTP control surface: window list, PNG
> grabs, real mouse/key/text injection, widget rects, log tail, graceful quit.
> It replaces `screencapture -l`, `winid.swift`, CGEvent scripting and the
> studio websocket bridge for all agent work. Full spec: [App Remote Control](#app-remote-control---remote).

## Execution Policy

- **Designs stay local.** Design documents, plans, and reports are local files
  (`local/agent_state/<topic>/DESIGN.md`) that lanes read from disk. Never
  publish them to the web (no Artifacts, no hosted pages); summarize in the
  terminal instead.
- Launch UI programs as standalone release binaries from this checkout. Do
  not use the Studio remote bridge, `ObserveMount`, `RunItem`, or any
  `cargo-makepad studio` websocket client.
- Launch with `--remote` whenever you intend to look at or drive the app,
  and finish with `GET /gq`. **Nothing of yours may outlive your task** —
  never leave a test window on the user's screen.
- Always use release builds for runtime validation, profiling, benchmarks,
  timing checks, or any performance-sensitive command. Use `--release`
  unless the user explicitly asks for a debug build.
- Build with `cargo build --release -p <package>`, then launch the
  resulting executable so its provenance is unambiguous. Do not use raw
  `cargo run` / `cargo makepad` to start a UI you will keep inspecting.
- Stop or replace an older standalone instance of the same target before
  launching a freshly built one.
- Keep an interactive standalone app running when the user asks to play
  with it. Use a separate self-terminating capture run only when a
  screenshot is also needed.
- `cargo check` or `cargo build` never counts as UI verification. After
  changing UI/runtime code, rebuild and relaunch before trusting what you
  see. Do not keep inspecting an older already-running binary.
- Command-line-only tasks (builds, tests, linting, file ops, grep, etc.)
  can be run directly in the shell.
- A standalone app's built-in screenshot/capture hook is valid for visual
  inspection.
- **System-level screenshots are FORBIDDEN.** Never run `screencapture`,
  `CGWindowListCreateImage`/`CGDisplayCreateImage` scripts, `xcap`,
  `import`, `scrot`, `grim`, `xwd`, PowerShell/Win32 screen grabs, or any
  other OS screen capture — not of the display, not of a window, not
  "just the caption". The user's screen is private. The only image of a
  running app you may ever take is the app's own `--remote` grab (`/g`,
  `/gq`, `/tweak/grab`), which renders the app's own drawable and nothing
  else. If something only shows in the OS layer (native caption buttons,
  other apps, the desktop), ask the user for a screenshot instead of
  taking one.
- When adding a new example crate, update both the Cargo workspace and
  `makepad.splash`.

## Standalone Launch
1. `cargo build --release -p <package>` from this checkout.
2. Kill any older process of that same executable.
3. Run `target/release/<bin> --remote` from the repo root (so resource paths
   resolve), parse the port from the startup line, drive it over HTTP.
4. After code changes, repeat 1–3 before drawing conclusions.
5. `GET /gq` when you are done. Always.

## App Remote Control (`--remote`)

> **Focus law.** A `--remote` app opens its window VISIBLE BUT UNFOCUSED and
> stays that way: it never activates, never becomes key, and bridge clicks
> never raise it. The user keeps typing wherever they were. Everything the
> bridge does (grabs, `/m`, `/k`, `/t`, `/snap`) works without focus because
> input is injected through the app's event loop, not the OS. Do not work
> around this (`MAKEPAD_FOCUS=1` exists only for a run the user asks to see
> in front); `MAKEPAD_NO_FOCUS=1` gives a non-remote launch the same manners.

> **Who may open a visible window.** Subagent/lane verification runs HIDDEN:
> launch with `MAKEPAD_HIDE_WINDOWS=1 <bin> --remote` — the window never
> appears, grabs (`/g`), `/snap`, `/m`, `/k`, `/t` all still work offscreen.
> Only the integrating session opens the one visible, unfocused window the
> user watches; several look-alike windows on screen made the user "go
> insane" (2026-08-26).


Any makepad app launched with `--remote` runs a localhost HTTP server inside
the process and prints one line before the UI appears:

```
[makepad-remote] listening on 127.0.0.1:53412 pid=9931 app=makepad-example-splash grabs=/var/folders/…/T/makepad-remote/makepad-example-splash-9931
```

Port, pid, app name and the grab directory — everything needed to drive and
clean up the instance, with no discovery step. `--remote=PORT` pins the port;
`MAKEPAD_REMOTE=1` (or `=PORT`) does the same via the environment. No app code
is involved: it lives in `app_main!`, so every app gets it for free.

### Cheat sheet

Every route is a plain `GET`. Every answer is **one line of JSON** with short
keys and real numbers. Errors are `{"err":"..."}` with HTTP 404.
`GET /` returns this table as plain text, so an agent that finds the port
learns the whole API in one request.

| Route | Answer | Notes |
|---|---|---|
| `/` `/help` | plain-text cheat sheet | self-describing; read this first |
| `/s` `?w=ID` | `{"app":…,"pid":…,"w":[{"i":0,"t":"Title","sz":[w,h],"px":[w,h],"dpi":2,"pos":[x,y]}]}` | `sz` = layout points, `px` = physical pixels |
| `/g` `?w=&scale=&raw=` | `{"png":"/abs/path.png","w":0,"sz":[w,h]}` | writes a file and returns the **path** (agents read images as files). `raw=1` sends `image/png` bytes instead. `scale=0.5` halves it |
| `/gq` `?w=&scale=` | `{"png":[paths…],"quit":1}` | **grab every window, then quit.** The canonical last call of a session |
| `/m` `?k=&x=&y=&w=&b=&dx=&dy=&wait=` | `{"ok":1,"f":frame}` | `k=move\|down\|up\|click\|scroll`; `b=0` left, `1` right, `2` middle |
| `/click` `?x=&y=&w=&wait=` | `{"ok":1}` | alias for `/m?k=click` (move + down + up) |
| `/k` `?t=TEXT` or `?k=down\|up\|press&c=CODE` | `{"ok":1}` | `t=` goes through the IME text path; `c=` takes `KeyA`/`a`/`enter`/`Escape`/`ArrowLeft`/`F1`/`Key1`… plus `&shift=1&ctrl=1&alt=1&cmd=1` |
| `/t` `?t=TEXT` | `{"ok":1}` | same as `/k?t=` |
| `/snap` `?q=&w=&all=` | `{"s":[{"i":"id","ty":"Button","r":[x,y,w,h],"w":0,"t":"Click me"}]}` | **how you find things to click.** `q=` filters id/type/text; rects are window-local, ready to feed to `/click` |
| `/d` `/dump` | plain text widget tree | one indented line per widget, ending `x y w h` |
| `/log` `?n=50&since=N` | `{"n":lastseq,"l":["[E] …"]}` | ring buffer of the app's own log output — see errors without owning stdout |
| `/close` `?w=ID` | `{"ok":1}` | closes one window the normal way |
| `/quit` | `{"ok":1}` | graceful shutdown, no final grab |

Add `&wait=1` to any input route to have it answer only **after the next frame
is drawn**, so a following `/g` sees the result with no `sleep`.
Add `&w=ID` to target a window; omit it for the first one.
`POST` the same routes with a flat JSON body (`{"x":10,"y":20}`) when quoting a
query string is painful; the key names are the long ones (`window`, `kind`,
`button`, `text`, `code`).

### The standard pattern

```bash
cargo build --release -p makepad-example-splash
./target/release/makepad-example-splash --remote > /tmp/app.log 2>&1 &
sleep 4
P=$(grep -o 'listening on 127.0.0.1:[0-9]*' /tmp/app.log | grep -o '[0-9]*$')

curl -s "http://127.0.0.1:$P/s"                      # {"app":…,"w":[{"i":0,…}]}
curl -s "http://127.0.0.1:$P/snap?q=press_demo"      # find the button's rect
curl -s "http://127.0.0.1:$P/click?x=352&y=472&wait=1"
curl -s "http://127.0.0.1:$P/snap?q=press_status"    # assert the app reacted
curl -s "http://127.0.0.1:$P/log?n=20"               # any errors?
curl -s "http://127.0.0.1:$P/gq?scale=0.5"           # final PNGs + quit
```

Read the returned `png` path with your image tool. `tools/remote_smoke.sh` is
this pattern as an executable end-to-end test across three example apps.

### Rules

- **Close what you open.** When you are done with an instance you launched,
  `GET /gq` (or `/close` each window, then `/quit`). Never leave test windows
  on the user's screen, and never `pkill` when the protocol is available.
- **Never touch an instance the user is running.** Launch your own.
- **`/g` is the only camera.** No OS-level screen capture of any kind (see
  Execution Policy) — the remote grab is what you get.
- **A vanished window or app with `[makepad-remote] user closed …` in the log
  means the human dismissed it — it was in their way.** Do **not** treat that
  as a crash and do **not** relaunch it. The app prints
  `[makepad-remote] user closed window 1 ("Inspector Panel")` and, when that
  was the last window, `[makepad-remote] app exit: user closed the last
  window`. Both lines go to stdout with or without `--remote`, and into the
  `/log` ring. While the app lives, `/s?w=1` on such a window answers
  `{"err":"window 1 closed by user"}` rather than "no window 1".
- **`--remote` windows are tagged.** Their title gets a ` [remote]` suffix
  (both the OS title bar and makepad's own caption bar) so a human who finds
  one lingering knows it is an agent instance and can close it guilt-free.
  `--remote-title-tag=NAME` changes the tag; `--remote-title-tag=off` removes
  it.

### Semantics worth knowing

- **Coordinates** are layout points, window-local, y down — the same space
  `MouseDownEvent.abs` uses, and the same space `/snap` reports rects in. No
  dpi maths: a rect from `/snap` goes straight into `/click`.
- **Window ids** are stable `usize` slots (`/s` `"i"`). Every window-targeting
  route takes `w=`; omitting it means the first created window. A request for
  a window that never existed 404s with `{"err":"no window 3"}`.
- **Input takes the real path.** Events are injected through
  `Cx::dispatch_studio_msg`, the same function the studio bridge uses, with
  the same `fingers` bookkeeping — so hits, capture, tap counts and gestures
  behave exactly as they do for a human. `/click` sends move + down + up so
  hover-dependent widgets see what they expect.
- **Grabs are real frames**, read back from the window's own presented
  drawable on the frame after the request (the studio screenshot pipeline,
  extended with per-window targeting). The UI thread is never blocked; the
  HTTP thread waits. Grabs are written to
  `$TMPDIR/makepad-remote/<app>-<pid>/grab-w<window>-<seq>.png`, monotonically
  numbered, with the last 32 per window retained.
- **Backends:** macOS/Metal is fully supported. Linux GL and Vulkan support
  grabs too. Windows/D3D11 has no screenshot readback yet, so `/g` there times
  out with `{"err":"grab timeout …"}` while every other route works. Android,
  OHOS and wasm compile to a no-op.
- **Cost when idle is zero.** The event loop only upshifts its paint clock
  while a remote request is in flight.

### The TWEAKER (`/tweak/*`) — design feedback and live styling

Every `--remote` app carries a design-feedback overlay (plan of record:
repo-root `tweaker.md`; implementation: `widgets/src/tweaker.rs`). Off it
costs nothing. On, the person (or you) points at the UI: pointer events over
the window body are swallowed before widget dispatch — **clicking a Button in
tweak mode outlines it and never fires it** — and the window grows a property
sidebar next to the (compressed) app UI. F12 toggles it in-app; every edit,
theirs or yours, lands in one shared diff log.

| Route | Answer | Notes |
|---|---|---|
| `/tweak` `?on=1\|0&annotate=1\|0` | `{"on":1,"annotate":0}` | toggle the overlay / the freehand draw mode (Alt-drag draws too) |
| `/tweak/state` | `{"on":1,"sel":{path,ty,r,band},"props":[{n,v,set}],"hover":…,"diff":[…],"ann":[…]}` | the STRUCTURE feedback: pinned selection, its real reflected properties (`set:1` = explicitly applied), the edit log, annotation strokes with the widget paths they touch |
| `/tweak/apply` (POST) | `{"ok":1,"path":…,"changed":[{path,prop,old,new}]}` | body `{"path":"a.b.c","splash":"{padding: Inset{left: 20}}"}` or the one-property shorthand `{"path":…,"prop":"draw_bg.border_radius","value":"8"}`. Evaluates the chunk onto that ONE instance through the ordinary apply machinery (`+:` merge rules intact) and triggers a full relayout. Answers after the next drawn frame |
| `/tweak/diff` | `{"diff":[{path,prop,old,new}…]}` | the raw edit log, in order |
| `/tweak/clear` | `{"ok":1}` | reset diff + annotations |
| `/tweak/final` | `{"final":[…coalesced…],"ann":[…],"drew":0\|1,"png":path?}` | **read this when tweaking is done**: per (path, prop) only the original and final value, churn collapsed. When the user drew, `png` is the composited screenshot — look at it, the strokes mean something |
| `/tweak/grab` | like `/g` | the overlay (outlines, strokes, sidebar) draws in the window's own pass, so any grab is already composited |

`local/tools/tweak` wraps all of this:
`tweak PORT on`, `tweak PORT state`, `tweak PORT apply PATH PROP VALUE`,
`tweak PORT splash PATH 'CHUNK'`, `tweak PORT final`, …

**How to listen.** Sidebar edits push to you: each one emits a marked
`TWEAK sidebar <path> <prop> <old> -> <new>` line into the app log — the
`/log` tail is your ear; you never poll `/tweak/state` for changes. Talk back
on `/tweak/apply` (values or whole shader chunks) to the same selected
instance.

**Write-back (you do this part — the overlay never writes source).** When the
session is done, take `/tweak/final` and edit the splash source:

1. Resolve each entry's widget path to its DSL site: the dotted path mirrors
   the `script_mod!` tree (`/d` shows the same ids). `-` segments are
   anonymous containers — skip them when searching the source.
2. Write each property at the **most specific existing site** — the widget's
   own `name := Type{…}` block if it has one; create one only when none
   exists.
3. Respect the merge law: a property inside a typed sub-struct goes through
   `+:` (`draw_bg +: { border_radius: 8 }`), never a replacing
   `draw_bg: {…}`. Plain walk/layout values (`padding`, `margin`, `width`)
   are set directly (`padding: Inset{left: 20}`).
4. Values come back in source spelling (`#rrggbbaa` colors, plain numbers) —
   paste them as-is. Mind the Rust-tokenizer hex-`e` trap in `script_mod!`:
   `#1e1e2e` must be written `#x1e1e2e`.
5. Rebuild and relaunch; verify the value survived with `/tweak/state` or
   `/snap` before calling it done.

Reflection truth: `props` come from the widget's live Rust fields plus the
type's DSL-declared shader inputs (`instance()`/`uniform()`), so the list is
what the widget actually exposes — there is no synthetic schema to drift.

### Studio remote bridge (the older path)

The studio (`studio/desktop` + `studio/hub`) drives a hosted app over a
websocket with the `StudioToApp` / `AppToStudio` protocol
(`platform/studio/src/studio.rs`): `MouseDown/Up/Move/Scroll`, `KeyDown/Up`,
`TextInput`, `TextCopy/Cut`, `GameInput`, `Screenshot`, `RunViewFrameRequest`,
`WidgetTreeDump`, `WidgetQuery`, `WidgetSnapshot`, `LiveChange`, `Custom`,
`Kill`, plus the shared-swapchain messages `Swapchain` / `WindowGeomChange` /
`Tick`. `libs/makepad_test` is the programmatic client for it
(`TestApp::try_click_center`, `try_type_text`, `try_screenshot`, …) and
`examples/*/tests/ui.rs` are its test suites.

`--remote` reuses that vocabulary — the same message types, the same injection
function, the same screenshot pipeline — but exposes it as HTTP on the app
itself, with no studio, no hub, no build ids, and with per-window targeting
that the studio path lacks. Use `--remote` for agent work; the studio bridge
remains for the studio and for `libs/makepad_test`.

## CLAUDE.md Body
The following is the current body of CLAUDE.md included verbatim for agent guidance parity.

# Makepad Project Guide

## Important: When Converting Syntax

**Always search for existing usage patterns in the NEW crates (widgets, code_editor, studio) before making syntax changes.** The old `widgets` and `live_design!` syntax is deprecated. When unsure about the correct syntax for something, grep for similar usage in `widgets/src/` to find the correct pattern.

```bash
# Example: find how texture declarations work in new system
grep -r "texture_2d" widgets/src/
```

**Critical: Always use `Name: value` syntax, never `Name = value`.** The old `Key = Value` syntax no longer works. For named widget instances, use `name := Type{...}` syntax.

## Running UI Programs

Launch UI apps as standalone release binaries from this checkout. Do not
use the Studio remote bridge.

```bash
cargo build --release -p makepad-app-asset-ui
# stop any older instance of the same binary, then:
./target/release/makepad-app-asset-ui
```

For one-shot visual smoke of a small example:

```bash
RUST_BACKTRACE=1 cargo run -p makepad-example-splash --release & PID=$!; sleep 15; kill $PID 2>/dev/null; echo "Process $PID killed"
```

To look at or drive a running app, add `--remote`: the app serves a localhost
HTTP control surface (window list, PNG grabs, real mouse/key/text injection,
widget rects, log tail) and prints its port on startup. Finish every session
with `GET /gq`, which grabs each window and quits — never leave a test window
on screen. Full protocol: repo-root `AGENTS.md`.

```bash
./target/release/makepad-example-splash --remote > /tmp/app.log 2>&1 &
P=$(grep -o 'listening on 127.0.0.1:[0-9]*' /tmp/app.log | grep -o '[0-9]*$')
curl -s "http://127.0.0.1:$P/"          # cheat sheet
curl -s "http://127.0.0.1:$P/gq"        # final grab + quit
```

When measuring runtime or performance, prefer `--release`.

## Cargo.toml Setup

```toml
[package]
name = "makepad-example-myapp"
version = "0.1.0"
edition = "2021"

[dependencies]
makepad-widgets = { path = "../../widgets" }
```


## Widgets DSL (script_mod!)

The new DSL uses `script_mod!` macro with runtime script evaluation instead of the old `live_design!` compile-time macros.

### Imports and App Setup

```rust
use makepad_widgets::*;

app_main!(App);

script_mod!{
    use mod.prelude.widgets.*
    
    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(800, 600)
                body +: {
                    // UI content here
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);  // Register all widgets
        // Platform-specific initialization goes here (e.g., vm.cx().start_stdin_service() for macos)
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Handle widget actions
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
```

### Available Widgets (widgets/src/lib.rs)

Core: `View`, `SolidView`, `RoundedView`, `ScrollXView`, `ScrollYView`, `ScrollXYView`
Text: `Label`, `H1`, `H2`, `H3`, `LinkLabel`, `TextInput`
Buttons: `Button`, `ButtonFlat`, `ButtonFlatter`
Toggles: `CheckBox`, `Toggle`, `RadioButton`
Input: `Slider`, `DropDown`
Layout: `Splitter`, `FoldButton`, `FoldHeader`, `Hr`
Lists: `PortalList`
Navigation: `StackNavigation`, `ExpandablePanel`
Overlays: `Modal`, `Tooltip`, `PopupNotification`
Dock: `Dock`, `DockSplitter`, `DockTabs`, `DockTab`
Media: `Image`, `Icon`, `LoadingSpinner`
Special: `FileTree`, `PageFlip`, `CachedWidget`
Window: `Window`, `Root`
Markup: `Html`, `Markdown` (feature-gated)

### Widget Definition Pattern

```rust
// Rust struct
#[derive(Script, ScriptHook, Widget)]
pub struct MyWidget {
    #[source] source: ScriptObjectRef,  // Required for script integration
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[live] draw_text: DrawText,
    #[rust] my_state: i32,  // Runtime-only field
}

// For widgets with animations, add Animator derive:
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct AnimatedWidget {
    #[source] source: ScriptObjectRef,
    #[apply_default] animator: Animator,
    // ...
}
```

### Script Module Structure

```rust
script_mod!{
    use mod.prelude.widgets_internal.*  // For internal widget definitions
    use mod.widgets.*                    // Access other widgets
    
    // Register base widget (connects Rust struct to script)
    mod.widgets.MyWidgetBase = #(MyWidget::register_widget(vm))
    
    // Create styled variant with defaults
    mod.widgets.MyWidget = set_type_default() do mod.widgets.MyWidgetBase{
        width: Fill
        height: Fit
        padding: theme.space_2
        
        draw_bg +: {
            color: theme.color_bg_app
        }
    }
}
```

### Key Syntax Differences (Old vs New)

| Old (live_design!) | New (script_mod!) |
|-------------------|-------------------|
| `<BaseWidget>` | `mod.widgets.BaseWidget{ }` |
| `{{StructName}}` | `#(Struct::register_widget(vm))` |
| `(THEME_COLOR_X)` | `theme.color_x` |
| `<THEME_FONT>` | `theme.font_regular` |
| `instance hover: 0.0` | `hover: instance(0.0)` |
| `uniform color: #fff` | `color: uniform(#fff)` |
| `draw_bg: { }` (replace) | `draw_bg +: { }` (merge) |
| `default: off` | `default: @off` |
| `fn pixel(self)` | `pixel: fn()` |
| `item.apply_over(cx, live!{...})` | `script_apply_eval!(cx, item, {...})` |

### Runtime Property Updates with script_apply_eval!

Use `script_apply_eval!` macro to dynamically update widget properties at runtime:
```rust
// Old system (live! macro with apply_over)
item.apply_over(cx, live!{
    height: (height)
    draw_bg: {is_even: (if is_even {1.0} else {0.0})}
});

// New system (script_apply_eval! macro)
script_apply_eval!(cx, item, {
    height: #(height)
    draw_bg: {is_even: #(if is_even {1.0} else {0.0})}
});

// For colors, use #(color) syntax
let color = self.color_focus;
script_apply_eval!(cx, item, {
    draw_bg: {
        color: #(color)
    }
});
```

Note: In `script_apply_eval!`, use `#(expr)` for Rust expression interpolation instead of `(expr)`.

### Theme Access

Always use `theme.` prefix:
```rust
color: theme.color_bg_app
padding: theme.space_2
font_size: theme.font_size_p
text_style: theme.font_regular
```

### Property Merging with `+:`

The `+:` operator merges with parent instead of replacing:
```rust
mod.widgets.MyButton = mod.widgets.Button{
    draw_bg +: {
        color: #f00  // Only overrides color, keeps other draw_bg properties
    }
}
```

### Shader Instance vs Uniform

- `instance(value)` - Per-draw-call value (can vary per widget instance)
- `uniform(value)` - Shared across all instances using same shader

```rust
draw_bg +: {
    hover: instance(0.0)           // Each button has its own hover state
    color: uniform(theme.color_x)  // Shared base color
    color_hover: instance(theme.color_y)  // Per-instance if color varies
}
```

### Animator Definition

```rust
animator: Animator{
    hover: {
        default: @off
        off: AnimatorState{
            from: {all: Forward {duration: 0.1}}
            apply: {
                draw_bg: {hover: 0.0}
                draw_text: {hover: 0.0}
            }
        }
        on: AnimatorState{
            from: {all: Snap}  // Instant transition
            apply: {
                draw_bg: {hover: 1.0}
                draw_text: {hover: 1.0}
            }
        }
    }
}
```

### Shader Functions

```rust
draw_bg +: {
    pixel: fn() {
        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
        sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 4.0)
        sdf.fill(self.color.mix(self.color_hover, self.hover))
        return sdf.result
    }
}
```

Note: Use `.method()` not `::method()` in shaders.

### Color Mixing (Method Chaining)

```rust
// Old nested style (avoid)
mix(mix(mix(color1, color2, hover), color3, down), color4, focus)

// New chained style (preferred)
color1.mix(color2, hover).mix(color3, down).mix(color4, focus)
```

### App Structure Pattern

```rust
script_mod!{
    use mod.prelude.widgets.*
    
    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1000, 700)
                body +: {
                    // Your UI here
                    MyWidget{}
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        // Platform-specific initialization (e.g., vm.cx().start_stdin_service() for macos)
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(ids!(my_button)).clicked(actions) {
            log!("Button clicked!");
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
```

### Widget ID References

Use `:=` for named widget instances:
```rust
// In DSL
my_button := Button{text: "Click"}

// In Rust code
self.ui.button(ids!(my_button)).clicked(actions)
```

### Template Definitions in Dock

Templates inside Dock are local; use `let` bindings at script level for reusable components:
```rust
script_mod!{
    // Reusable at script level
    let MyPanel = SolidView{
        width: Fill
        height: Fill
        // ...
    }
    
    // Use directly
    body +: {
        MyPanel{}  // Works because it's a let binding
    }
}
```

### Custom Draw Widget Example

```rust
#[derive(Script, ScriptHook, Widget)]
pub struct CustomDraw {
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_quad: DrawQuad,
    #[rust] area: Area,
}

impl Widget for CustomDraw {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_quad.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}
```

### Script Object Storage: map vs vec

In script objects, properties are stored in two different places:
- **`map`**: Contains `key: value` pairs (regular properties)
- **`vec`**: Contains named template items (via `:=` syntax)

This distinction is important when working with `on_after_apply` or inspecting script objects directly.

### Templates in List Widgets (PortalList, FlatList)

In list widgets, named IDs (using `:=`) define **templates** that are stored in the widget's `templates` HashMap. These are NOT regular properties - they go into the script object's vec and are collected via `on_after_apply`.

```rust
// In script_mod! - defining templates for a list
my_list := PortalList {
    // Regular properties (go into struct fields)
    width: Fill
    height: Fill
    scroll_bar: mod.widgets.ScrollBar {}
    
    // Templates (named with :=) - stored in templates HashMap, NOT struct fields
    Item := View {
        height: 40
        title := Label { text: "Default" }
    }
    Header := View {
        draw_bg: { color: #333 }
    }
}
```

The templates are collected in `on_after_apply`:
```rust
impl ScriptHook for PortalList {
    fn on_after_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, scope: &mut Scope, value: ScriptValue) {
        if let Some(obj) = value.as_object() {
            vm.vec_with(obj, |_vm, vec| {
                for kv in vec {
                    if let Some(id) = kv.key.as_id() {
                        self.templates.insert(id, kv.value);
                    }
                }
            });
        }
    }
}
```

Then used during drawing:
```rust
while let Some(item_id) = list.next_visible_item(cx) {
    let item = list.item(cx, item_id, id!(Item));
    item.label(ids!(title)).set_text(cx, &format!("Item {}", item_id));
    item.draw_all(cx, &mut Scope::empty());
}
```

**Key distinction**: Regular properties like `scroll_bar: mod.widgets.ScrollBar {}` are applied directly to struct fields. Template definitions like `Item := View {...}` are stored separately for dynamic instantiation.

### PortalList Usage

```rust
#[derive(Script, ScriptHook, Widget)]
pub struct MyList {
    #[deref] view: View,
}

impl Widget for MyList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, 100);  // 100 items
                
                while let Some(item_id) = list.next_visible_item(cx) {
                    let item = list.item(cx, item_id, id!(Item));
                    item.label(ids!(title)).set_text(cx, &format!("Item {}", item_id));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
}
```

### FileTree Usage

```rust
impl Widget for FileTreeDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree.set_folder_is_open(cx, live_id!(root), true, Animate::No);
            // Draw nodes recursively
            self.draw_node(cx, live_id!(root));
        }
        DrawStep::done()
    }
}
```

### Registering Custom Draw Shaders

For custom draw types with shader fields, use `script_shader`:

```rust
script_mod!{
    use mod.prelude.widgets_internal.*
    
    // Register custom draw shader
    set_type_default() do #(DrawMyShader::script_shader(vm)){
        ..mod.draw.DrawQuad  // Inherit from DrawQuad
    }
    
    // Register widget that uses it
    mod.widgets.MyWidgetBase = #(MyWidget::register_widget(vm))
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawMyShader {
    #[deref] draw_super: DrawQuad,
    #[live] my_param: f32,
}
```

### Registering Components (non-Widget)

For structs that aren't full widgets but need script registration:

```rust
script_mod!{
    // For components (not widgets)
    mod.widgets.MyComponentBase = #(MyComponent::script_component(vm))
    
    // For widgets (implements Widget trait)
    mod.widgets.MyWidgetBase = #(MyWidget::register_widget(vm))
}
```

### Script Prelude Modules

Two prelude modules available:
- `mod.prelude.widgets_internal.*` - For internal widget library development
- `mod.prelude.widgets.*` - For app development (includes all widgets)

```rust
script_mod!{
    // App development - use widgets prelude
    use mod.prelude.widgets.*
    
    // Or for widget library internals
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
}
```

### Default Enum Values

For enums with a `None` variant that need `Default`, use standard Rust `#[default]` attribute instead of `DefaultNone` derive:

```rust
// Correct - use #[default] attribute on the None variant
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum MyAction {
    SomeAction,
    AnotherAction,
    #[default]
    None,
}

// Wrong - don't use DefaultNone derive
#[derive(Clone, Copy, Debug, PartialEq, DefaultNone)]  // Don't do this
pub enum MyAction {
    SomeAction,
    None,
}
```

### Multi-Module Script Registration Pattern

When refactoring a multi-file project (like studio) from `live_design!` to `script_mod!`:

1. **Each widget module** defines its own `script_mod!` that registers to `mod.widgets.*`:
```rust
// In studio_editor.rs
script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    
    mod.widgets.StudioCodeEditorBase = #(StudioCodeEditor::register_widget(vm))
    mod.widgets.StudioCodeEditor = set_type_default() do mod.widgets.StudioCodeEditorBase {
        editor := CodeEditor {}
    }
}
```

2. **The lib.rs** aggregates all widget script_mods:
```rust
pub fn script_mod(vm: &mut ScriptVm) {
    crate::module1::script_mod(vm);
    crate::module2::script_mod(vm);
    // ... all widget modules
}
```

3. **The app.rs** calls them in correct order:
```rust
impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);  // Base widgets first
        crate::script_mod(vm);                    // Your widget modules
        crate::app_ui::script_mod(vm);            // UI that uses the widgets
        App::from_script_mod(vm, self::script_mod)
    }
}
```

4. **The app_ui.rs** can then use registered widgets:
```rust
script_mod! {
    use mod.prelude.widgets.*
    // Now StudioCodeEditor is available from mod.widgets
    
    let EditorContent = View {
        editor := StudioCodeEditor {}
    }
}
```

### Cross-Module Sharing via `mod` Object

**IMPORTANT**: `use crate.module.*` does NOT work in script_mod. The `crate.` prefix is not available.

To share definitions between script_mod blocks in different files, store them in the `mod` object:

```rust
// In app_ui.rs - export to mod.widgets namespace
script_mod! {
    use mod.prelude.widgets.*
    
    // This makes AppUI available as mod.widgets.AppUI
    mod.widgets.AppUI = Window{
        // ...
    }
}

// In app.rs - import via mod.widgets
script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*  // Now AppUI is in scope
    
    load_all_resources() do #(App::script_component(vm)){
        ui: Root{ AppUI{} }
    }
}
```

The `mod` object is the only way to share data between script_mod blocks.

### Prelude Alias Syntax

When defining a prelude, use `name:mod.path` to create an alias:
```rust
mod.prelude.widgets = {
    ..mod.std,           // Spread all of mod.std into scope
    theme:mod.theme,     // Create 'theme' as alias for mod.theme
    draw:mod.draw,       // Create 'draw' as alias for mod.draw
}
```

Without the alias (just `mod.theme,`), the module is included but has no name - you can't access it!

### Let Bindings are Local

`let` bindings in script_mod are LOCAL to that script_mod block. They cannot be:
- Accessed from other script_mod blocks
- Used as property values directly (e.g., `content +: MyLetBinding` won't work)

To use a `let` binding, instantiate it: `MyLetBinding{}` or store it in `mod.*` for cross-module access.

### Debug Logging with `~`

Use `~expression` to log the value of an expression during script evaluation:
```rust
script_mod! {
    ~mod.theme           // Logs the theme object
    ~mod.prelude.widgets // Logs what's in the prelude
    ~some_variable       // Logs a variable's value (or "not found" error)
}
```

### Common Pitfalls

**Widget ID references**: Named widget instances use `:=` in the DSL and plain names in Rust id macros:
- DSL defines `code_block := View { ... }` → Rust uses `id!(code_block)`
- DSL defines `my_button := Button { ... }` → Rust uses `ids!(my_button)`

1. **Missing `#[source]`**: All Script-derived structs need `#[source] source: ScriptObjectRef`

2. **Template scope**: Templates defined inside Dock aren't available outside; use `let` at script level

3. **Uniform vs Instance**: Use `instance()` for per-widget varying colors (like hover states on backgrounds)

4. **Forgot `+:`**: Without `+:`, you replace the entire property instead of merging

5. **Theme access**: Always `theme.color_x`, never `THEME_COLOR_X` or `(theme.color_x)`

6. **Missing widget registration**: Call `crate::makepad_widgets::script_mod(vm)` in `App::run()` before your own `script_mod`. Note: the old `live_design!` system and its crates are archived under `old/`

7. **Draw shader repr**: Custom draw shaders need `#[repr(C)]` for correct memory layout

8. **DefaultNone derive**: Don't use `DefaultNone` derive - use standard `#[derive(Default)]` with `#[default]` attribute on the `None` variant

9. **Script_mod call order**: Widget modules must be registered BEFORE UI modules that use them. Always call `lib.rs::script_mod` before `app_ui::script_mod`

10. **`pub` keyword invalid in script_mod**: Don't use `pub mod.widgets.X = ...`, just use `mod.widgets.X = ...`. Visibility is controlled by the Rust module system, not script_mod.

11. **Syntax for Inset/Align/Walk**: Use constructor syntax - `margin: Inset{left: 10}` not `margin: {left: 10}`, `align: Align{x: 0.5 y: 0.5}` not `align: {x: 0.5, y: 0.5}`

12. **Cursor values**: Use `cursor: MouseCursor.Hand` not `cursor: Hand` or `cursor: @Hand`

13. **Resource paths**: Use `crate_resource("self://path")` not `dep("crate://self/path")`

14. **Texture declarations in shaders**: Use `tex: texture_2d(float)` not `tex: texture2d`

15. **Enums not exposed to script**: Some Rust enums like `PopupMenuPosition::BelowInput` may not be exposed to script. If you get "not found" errors on enum variants, just remove the property and use the default

17. **Shader `mod` vs `modf`**: The Makepad shader language uses `modf(a, b)` for float modulo, NOT `mod(a, b)`. Similarly, use `atan2(y, x)` not `atan(y, x)` for two-argument arctangent. `atan(x)` (single arg) is also available. `fract(x)` works as expected.

16. **Draw shader struct field ordering**: In `#[repr(C)]` draw shader structs that extend another draw shader via `#[deref]`, NEVER place `#[rust]` or other non-instance data AFTER `DrawVars` and the instance fields. The system uses an unsafe pointer trick in `DrawVars::as_slice()` that reads contiguously past the end of `dyn_instances` into the subsequent `#[live]` fields. Any non-instance data between `DrawVars` and the instance fields will corrupt the GPU instance buffer. Put all extra data (like `#[rust]`, `#[live]` non-instance fields such as resource handles, booleans, etc.) BEFORE the `#[deref]` field, and only `#[live]` instance fields (the ones that map to shader inputs) AFTER.
    ```rust
    // CORRECT - non-instance data before deref, instance fields after
    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct MyDrawShader {
        #[live] pub svg: Option<ScriptHandleRef>,  // non-instance, BEFORE deref
        #[rust] my_state: bool,                     // non-instance, BEFORE deref
        #[deref] pub draw_super: DrawVector,        // contains DrawVars + base instance fields
        #[live] pub tint: Vec4f,                    // instance field, AFTER deref - OK
    }

    // WRONG - rust data after instance fields breaks the memory layout
    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct MyDrawShader {
        #[deref] pub draw_super: DrawVector,
        #[live] pub tint: Vec4f,      // instance field
        #[rust] my_state: bool,       // BAD: sits between tint and the next shader's fields
    }
    ```

18. **Don't put comments or blank lines before the first real code in `script!`/`script_mod!`**: Rust's proc macro token stream strips comments entirely — they produce no tokens. This shifts error column/line info because the span tracking starts from the first actual token. Always start with real code (e.g., `use mod.std.assert`) immediately after the opening brace.

19. **WARNING: Hex colors containing the letter `e` in `script_mod!`**: The Rust tokenizer interprets `e` or `E` in hex color literals as a scientific notation exponent, causing parse errors like `expected at least one digit in exponent`. For example, `#2ecc71` fails because `2e` looks like the start of `2e<exponent>`. **Use the `#x` prefix** to escape this: write `#x2ecc71` instead of `#x2ecc71`. This applies to any hex color where a digit is immediately followed by `e`/`E` (e.g., `#1e1e2e`, `#4466ee`, `#7799ee`, `#bb99ee`). Colors without `e` (like `#ff4444`, `#44cc44`) work fine with plain `#`.

20. **Shader enums**: Prefer `match` on enum values with `_ =>` as the catch-all arm, not `if/else` chains over integer-like values. If enum `match` fails in shader compilation, treat it as a compiler bug: add or extend a `platform/script/test` case and fix the shader compiler path instead of rewriting shader logic to `if/else`.
