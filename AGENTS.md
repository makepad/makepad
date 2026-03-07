# Studio Remote Runbook

## Execution Policy
- Visual UI programs must be launched and controlled through the Makepad Studio remote protocol.
- Command-line-only tasks (builds, tests, linting, file ops, grep/ripgrep, etc.) can be run directly in the shell.
- Prefer studio remote control for any workflow that needs screenshots, widget queries, clicks, typing, or runtime UI inspection.
- Before using Studio protocol tools (`FindInFiles`, `ReadTextRange`, `WidgetTreeDump`, `WidgetQuery`, `Screenshot`, `Click`, `TypeText`, `Return`), always start one persistent `cargo makepad studio` background process and reuse it for the entire interaction.

## Assumptions
- Studio is started manually by the user.
- Studio remote target is `ip:port` only (no `http://`, no `ws://`), normally `127.0.0.1:8001`.
  - Use `127.0.0.1:8002` only if Studio reports fallback because `8001` is occupied.
- Keep one persistent studio remote process for the whole interaction.

## Start Studio Remote
- Command:
  - `cargo run -p cargo-makepad --release -- studio --studio=127.0.0.1:8001`
  - Explicit mode: `cargo run -p cargo-makepad --release -- studio studio_remote --studio=127.0.0.1:8001`
- Send newline-delimited JSON requests on stdin.
- Read newline-delimited JSON responses on stdout.
- Protocol shape is `UIToStudio` requests and filtered `StudioToUI` responses.

## Request Protocol (JSON Lines)
- `{"ListBuilds":[]}`
- `{"LoadRunnableBuilds":{"mount":"makepad"}}`
- `{"Cargo":{"mount":"makepad","args":["check","-p","makepad-example-splash"],"env":null,"buildbox":null}}`
- `{"Run":{"mount":"makepad","process":"makepad-example-splash","args":[]}}`
- `{"Run":{"mount":"makepad","process":"makepad-example-splash","args":["--my-app-arg"]}}`
- `{"FindInFiles":{"mount":"makepad","pattern":"UIToStudio::","is_regex":false,"glob":null,"max_results":200}}`
- `{"FindInFiles":{"mount":"makepad","pattern":"UIToStudio::(FindInFiles|ReadTextRange)","is_regex":true,"glob":"**/*.rs","max_results":200}}`
- `{"ReadTextRange":{"path":"makepad/studio/backend/src/dispatch.rs","start_line":640,"end_line":720}}`
- `{"StopBuild":{"build_id":38160721318170}}`
- `{"WidgetTreeDump":{"build_id":38160721318170}}`
- `{"WidgetQuery":{"build_id":38160721318170,"query":"id:todo_input"}}`
- `{"Screenshot":{"build_id":38160721318170,"kind_id":0}}` (`kind_id` optional; defaults to `0`)
- `{"Click":{"build_id":38160721318170,"x":1274,"y":342}}`
- `{"TypeText":{"build_id":38160721318170,"text":"hello"}}`
- `{"Return":{"build_id":38160721318170,"auto_dump":false}}`
- `{"ForwardToApp":{"build_id":38160721318170,"msg_bin":[...]}}` (advanced; binary payload)

## `StudioToApp` API (Updated)
- The studio remote bridge supports raw app event passthrough via `UIToStudio::ForwardToApp`.
- Current `StudioToApp` variants include:
  - `Screenshot`, `WidgetTreeDump`, `KeepAlive`, `LiveChange`, `Swapchain`, `WindowGeomChange`, `Tick`
  - `MouseDown`, `MouseUp`, `MouseMove`, `Scroll`
  - `KeyDown`, `KeyUp`, `TextInput`, `TextCopy`, `TextCut`
  - `None`, `Kill`
- Use direct studio remote requests (`Screenshot`, `WidgetTreeDump`, `Click`, `TypeText`, `Return`) for normal automation.
- Use raw `StudioToApp` only for low-level event injection/debugging.

## Response Notes (Current)
- Bridge stdout is filtered to: `Hello`, `Error`, `TextFileRead`, `TextFileRange`, `FindFileResults`, `SearchFileResults`, `Builds`, `RunnableBuilds`, `BuildStarted`, `BuildStopped`, `Screenshot`, `WidgetTreeDump`, `WidgetQuery`, `QueryCancelled`.
- `TerminalOutput`/`TerminalOpened`/`TerminalExited` and all `RunView*` messages are suppressed by `cargo makepad studio`.
- `Screenshot` responses include file metadata (`path`, `width`, `height`) and not inline PNG bytes.
- `WidgetTreeDump` responses include text dump content keyed by `request_id`.
- `FindInFiles` responds as `SearchFileResults` with concise entries (`path`, `line`, `column`, `line_text`) and `done`.
- `FindInFiles` defaults to searching only `.rs`, `.md`, `.toml` files unless `glob` is provided.
- `ReadTextRange` responds as `TextFileRange` with `path`, requested `start_line`/`end_line`, `total_lines`, and `content`.
- Query-scoped responses are lane-filtered by `query_id.client_id`; only this bridge client's query results are emitted.
- `FindInFiles`/`SearchFiles` execution is worker-pooled in backend (not main dispatch thread).

## Recommended Control Flow
1. Start studio remote process once.
2. For code search, use `FindInFiles` first, then `ReadTextRange` to window exact regions.
3. Prefer `Run` for app execution and wait for `BuildStarted`.
4. Use `Cargo` only for raw cargo passthrough commands.
5. Use `WidgetQuery` / `WidgetTreeDump` to get click targets.
6. For text input, click field first, then send text, then return.
7. Keep control packets compact (`auto_dump:false` on click/type/return for low latency).

## `Run` Defaults
- `Run` always builds a `cargo run -p <process>` command with `--release`.
- `Run` always sets cargo `--message-format=json`.
- `Run.args` are app args placed after `--`.
- `Run.standalone` is optional; default is `false` (in-Studio runview/framebuffer mode).
- `Run` injects app `--message-format=json` when missing.
- `Run` injects `--stdin-loop` unless `standalone:true`.

## `Cargo` Defaults
- `Cargo` is passthrough for `args`, but injects `--message-format=json` when missing for supported subcommands (`build`, `check`, `run`, `test`, `bench`, `rustc`).

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

```bash
RUST_BACKTRACE=1 cargo run -p makepad-example-splash --release & PID=$!; sleep 15; kill $PID 2>/dev/null; echo "Process $PID killed"
```

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
