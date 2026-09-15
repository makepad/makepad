# Splash and widget reference

Use this for current `script_mod!` work. Repository policy is in
[AGENTS.md](../../AGENTS.md). Check the implementation closest to your task
before copying a signature; the archived `live_design!` tree is not the
reference for current APIs.

## Working sources

| Task | Reference |
|---|---|
| Small app and startup | [Counter example](../../examples/counter/src/main.rs) |
| Broader widget examples | [Splash example](../../examples/splash/src/main.rs) |
| Widget registration and exports | [widgets/src/lib.rs](../../widgets/src/lib.rs) |
| Styling and animation | [Button](../../widgets/src/button.rs) |
| Widget lookup and updates | [Widget API](../../widgets/src/widget.rs) |
| Virtualized list templates | [PortalList](../../widgets/src/portal_list.rs) |
| Tree drawing and open state | [FileTree](../../widgets/src/file_tree.rs) |
| Generated widget APIs | [Widget derives](../../widgets/derive_widget/src/derive_widget.rs) |
| Shader instance memory | [DrawVars](../../platform/src/draw_vars.rs) |
| Script/compiler regression cases | [Script test crate](../../platform/script/test) |

Use the widget exports and source for availability and feature gates instead
of maintaining a separate widget catalogue here.

## App structure

A compact pattern using the current counter example's startup and lookup
APIs follows. Keep the registration style of the app you are editing.

```rust
pub use makepad_widgets;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(420, 220)
                body +: {
                    flow: Down
                    status := Label{text: "Ready"}
                    press := Button{text: "Press"}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(press)).clicked(actions) {
            self.ui.label(cx, ids!(status)).set_text(cx, "Pressed");
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
```

For a new example crate, use an adjacent crate's Cargo configuration and
update the workspace and `makepad.splash`. Startup helpers and app traits
can change; the source links above are the reference.

## Object syntax, scope, and registration

- Properties use `name: value`; named child/template instances use
  `name := Type{...}`.
- Assigning a module export uses `mod.widgets.Name = ...`. This is an
  assignment, not the old property syntax.
- App scripts import `mod.prelude.widgets.*`. Widget implementations use
  `mod.prelude.widgets_internal.*` and other required modules.
- `let` bindings are local to the script module. Export shared definitions
  through `mod.*`, then import that module. Rust's `crate.*` is not a
  Splash module path, and Rust `pub` is not used on Splash assignments.
- Register widgets with `#(Type::register_widget(vm))`, other components
  with `#(Type::script_component(vm))`, and draw shaders with
  `#(Type::script_shader(vm))`.
- Register dependencies before modules whose UI instantiates them. Preserve
  the host's existing app/module registration sequence.
- Derive fields and attributes depend on the type. Copy the nearest current
  implementation; `#[source]` is not a universal requirement for every
  Script-derived struct. Inspect the derive parser if a field type fails.

A widget's styled default commonly has this shape:

```rust
script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.MyButtonBase = #(MyButton::register_widget(vm))
    mod.widgets.MyButton = set_type_default() do mod.widgets.MyButtonBase{
        width: Fill
        height: Fit
        draw_bg +: {color: theme.color_bg_app}
    }
}
```

This assumes a matching Rust widget and draw fields are registered; it is
a pattern, not a standalone widget implementation.

## Values and property merging

Use `+:` for a partial override of an inherited typed object:

```rust
draw_bg +: {color: #f00}
padding: Inset{left: 10 top: 4}
align: Align{x: 0.5 y: 0.5}
```

Replacing a typed object can discard its other fields or methods. Walk and
layout values should use the constructor expected by the property.

Runtime property updates interpolate Rust values with `#(expr)`:

```rust
script_apply_eval!(cx, item, {
    height: #(height)
    draw_bg +: {color: #(color)}
});
```

Use `theme.*` for theme values. Resource references use `crate_resource`;
copy a nearby current resource path rather than translating an old
`dep("crate://...")` example mechanically. Cursor values use the exposed
enum, such as `MouseCursor.Hand`. If an enum is not exposed, inspect its
registration rather than silently discarding a required property.

Rust tokenizes code inside `script_mod!` before Splash sees it. Use escaped
colors such as `#x2ecc71` and `#x1e1e2e` where a digit followed by `e` or
`E` would otherwise be parsed as an exponent.

For Rust action enums, use `#[derive(Default)]` and `#[default]` on the
default variant, not the old `DefaultNone` derive.

## Templates and lists

Script objects distinguish ordinary `key: value` properties in their map
from named `name := Type{...}` items in their vector. List widgets collect
the latter as templates during application.

```rust
list := PortalList{
    width: Fill
    height: Fill
    Item := View{
        height: 40
        title := Label{text: "Default"}
    }
}
```

The current PortalList keeps templates as rooted `ScriptObjectRef` values,
collects them outside eval-only application, and updates instantiated items
on reload. Preserve those lifetime rules in custom list widgets.

During list drawing, use the current `set_item_range`, `next_visible_item`,
and `item` APIs. Widget lookup includes the context:

```rust
let item = list.item(cx, item_id, id!(Item));
item.label(cx, ids!(title)).set_text(cx, &format!("Item {item_id}"));
item.draw_all(cx, &mut Scope::empty());
```

For Dock-local templates, use a script-level `let` or module export when a
component must be reused outside that Dock. For custom widgets, turtle
drawing, FileTree traversal, and full animation state definitions, follow
the linked implementations instead of maintaining duplicate tutorials.

## Shaders and animation

- Per-draw shader values use `instance(value)`; shared values use
  `uniform(value)`. A varying hover color must not accidentally be uniform.
- Texture declarations use `texture_2d(float)`.
- Shader functions use `pixel: fn() {...}` and method calls use `.`, not
  Rust `::`. Chained `color.mix(other, amount)` is a useful styling idiom.
- Use `modf(a, b)` for float modulo and `atan2(y, x)` for the two-argument
  arctangent. `atan(x)` and `fract(x)` are also available.
- Animator states use id values such as `default: @off`. Use the current
  Button implementation for state definitions and transition constructors.
- Prefer enum `match` with a catch-all. A failure in supported shader enum
  matching needs a compiler regression case and fix, not an integer-branch
  workaround in the widget.

Custom draw shaders need `#[repr(C)]`. Keep non-instance fields before the
draw base and shader instance fields after it:

```rust
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct MyDrawShader {
    #[rust] cached_state: bool,
    #[deref] draw_super: DrawQuad,
    #[live] tint: Vec4f,
}
```

`DrawVars::as_slice()` reads across the base into trailing instance fields.
Putting non-instance state in that region corrupts the GPU instance buffer,
including when another draw shader extends yours. Follow the base shader's
current layout and registration.
