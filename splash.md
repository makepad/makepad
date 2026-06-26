# Splash DSL Guide

General Makepad Splash DSL patterns that apply to ANY app body.

## Key Rules

- **`let`/`fn` declarations must be at the top**, before any widget. The body starts with declarations, then the root widget.
- **Every container MUST have `height: Fit`** — most common failure mode. Inside a fixed-height parent, `height: Fill` is fine.
- **Root container MUST use `width: Fill`** — never a fixed pixel width. The app renders inside a parent container that provides the width.
- `ui` object is built-in; do NOT declare it with `:=`
- **`for` loops render widgets at build time only** — array changes do NOT re-render. Use `set_text()` for dynamic content.
- **Functions with `for` loops return empty strings** when called from `on_click` — inline string building instead
- **`as int` type casting produces NaN** — use string display + `set_text()` only
- **Colons inside string arguments work correctly** — `"Time: 2:30"` is fine
- Every `TextInput` must have a fixed numeric height (e.g. `34`)
- No `on_render` in embedded apps
- **Array / argument / object-body items may be separated by whitespace, newlines, OR commas — all work.** Adjacent values like `{a:1} {b:2}` (or one per line) are TWO separate items. Two object literals next to each other are NEVER "object inherits from object" — to extend/merge an object use `base += {field: val}` or a named prototype `Proto{...}`, not bare `{...}{...}`. (So `let days = [{...} {...} {...}]` correctly yields a 3-element array; a stray earlier bug collapsed comma-less object arrays to length 1.)

## Widget Availability

**Available:** View, RoundedView, Label, TextInput, LinkLabel, Button, ButtonFlat, ButtonFlatter, Slider, CheckBox, CheckBoxFlat, RadioButton, RadioButtonFlat, ToggleFlat, DropDown, TabBar, Tab, PopupMenu, ScrollBar, ScrollBars, LoadingSpinner, Hr, Vr, Icon

**NOT available (silently fail):** Stack, Divider, ProgressBar, IconButton, ToggleButton, Image, ListView, Grid, ColorPicker, ScrollPair

| Wanted | Not Available | Use Instead |
|--------|--------------|-------------|
| Divider line | `Divider` | `Hr{height:1 width:Fill}` |
| Progress bar | `ProgressBar` | `Slider{value:0.65 is_read_only:true}` |
| Tabbed UI | `TabBar`/`Tab` | `ButtonFlat` rows (TabBar renders zero-size) |

## Styling Gotchas

**`draw_bg.border_radius` takes a float, not an Inset:**
```splash
// ✅
draw_bg.border_radius: 16.0

// ❌ parse error — silently breaks layout
draw_bg.border_radius: Inset{top:0 bottom:16 left:0 right:0}
```

**`#x` prefix for hex colors containing 'e':** When a hex color contains the letter `e` adjacent to digits (like `#1e1e2e`), use `#x` to avoid parser ambiguity. Without `#x`, Makepad's parser may misinterpret digits following 'e' as an exponent:
```splash
#x2ecc71     // ✅ contains 'e' next to digits, use #x
#x1e1e2e     // ✅ contains 'e' next to digits, use #x
#ff4444      // ✅ no 'e' issue, plain # works
#00ff00      // ✅ no 'e' issue
```

**Default text color is white:** All text widgets (`Label`, `Button`, etc.) default to `#fff`. For light/white backgrounds, you MUST explicitly set `draw_text.color` to a dark color on every text element:
```splash
RoundedView{draw_bg.color:#f5f5f5 height:Fit
  Label{text:"Visible!" draw_text.color:#x222}
}
```

**Label styling shorthand:** Both syntaxes work:
```splash
Label{text:"Hello" color:#x2ecc71 font_size:16}              // bare props work
Label{text:"Hello" draw_text.color:#x2ecc71 draw_text.text_style.font_size:16}  // draw_text also works
```

**`new_batch: true` for text visibility:** Required on any container with `show_bg: true` that contains text children. Without it, text renders behind the background (invisible):
```splash
// ✅ Correct
RoundedView{width:Fill height:Fit new_batch:true show_bg:true draw_bg.color:#x334
  Label{text:"Visible" draw_text.color:#fff}
}
// ❌ Text may be invisible (draws behind bg)
RoundedView{width:Fill height:Fit show_bg:true draw_bg.color:#x334
  Label{text:"Invisible!" draw_text.color:#fff}
}
```

## Widget Reliability Reference

| Widget | Capabilities | Best For |
|--------|-------------|----------|
| **`ButtonFlat`** | Click → variable write, `set_text()`, `text()` | All interactive controls |
| **`Button`** | Click → variable write, `set_text()`, `text()` | Standard buttons |
| **`Label`** | `set_text()` updates visible text, `text()` reads back | Display values, status, dynamic list display |
| **`TextInput`** | `type_text` fills first input, `text()` reads value, `set_text()` writes | Text entry |
| **`Hr`** | Full-width line divider | Visual separation |
| **`RoundedView`** | Container with rounded corners | App root, groups |

## Splash VM Variable Scope

**`let` variables DO persist** across click events in the same app session:
- Counter: `let count = 0; count = count + 1` correctly produces `1, 2, 3, 4` across consecutive clicks
- Toggle: `let toggled = false; toggled = !toggled` persists `true` state across separate button clicks

However, **widget `checked` state** on `RadioButton`, `ToggleFlat`, `CheckBox` does NOT persist because internal post-processing discards the `on_click` scope context.

| Widget | Visual State | Variable Persistence |
|--------|-------------|---------------------|
| **`RadioButton`** | `checked: true` in widget tree | ❌ Lost — internal post-processing discards `on_click` scope |
| **`ToggleFlat`** | `checked` visual renders | ❌ Same limitation |
| **`CheckBox`** / **`CheckBoxFlat`** | `checked: true` in widget tree | ❌ Same limitation |

**Use `ButtonFlat` with manual toggle for persistent boolean state:**
```splash
let toggled = false
ButtonFlat{text:"Toggle" on_click:||{toggled = !toggled; ui.display.set_text("" + toggled)}}
ButtonFlat{text:"Show" on_click:||{ui.display.set_text("Current: " + toggled)}}
```

## Patterns

### Struct Arrays & Array Operations

The Splash VM supports arrays of structs with `.push()`, `.remove()`, `.len()`, and `.retain()`. Read fields via `array[index].field`, update with `array[index] += {field: val}`:

```splash
let items = [
    {text: "Task 1" tag: "work" done: false}
    {text: "Task 2" tag: "personal" done: false}
]
let max_items = 5

fn add_item(text){
    let clean = ("" + text).trim()
    if clean == "" { return }
    if items.len() >= max_items { return }
    items.push({text: clean tag: "" done: false})
    sync_all()
}

fn toggle_item(index){
    if index >= items.len() { return }
    items[index] += {done: !items[index].done}
    sync_all()
}

fn remove_item(index){
    if index >= items.len() { return }
    items.remove(index)
    sync_all()
}

fn clear_flagged(){
    items.retain(|it| !it.done)
    sync_all()
}
```

### Component / Template Pattern

Define reusable templates with `let` and instantiate with property overrides:

```splash
let ItemRow = RoundedView{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    new_batch: true
    draw_bg.color: #x2a2a3a
    draw_bg.border_radius: 8.0
    label := Label{text: "item" width: Fill draw_text.color: #xddd}
    action := ButtonFlatter{text: "Do" width: 56 height: 28}
    remove := ButtonFlatter{text: "X" width: 56 height: 28}
}

row_0 := ItemRow{
    label.text: "First item"
    action.on_click: || do_something(0)
    remove.on_click: || remove_item(0)
}
```

Override syntax: `<child-name>.<property>: <value>` — every segment in the path must use `:=`.

### Pre-allocated Fixed Slots

`for` loops render at build-time only — array changes don't add/remove widgets. Pre-allocate a fixed number of rows and update via sync functions:

```splash
let items = [{text: "Item 1"} {text: "Item 2"}]

fn sync_row_0(){
    if 0 < items.len() {
        ui.row_0.label.set_text(items[0].text)
    } else {
        ui.row_0.label.set_text("Empty slot")
    }
}
fn sync_rows(){
    sync_row_0()
    sync_row_1()
    sync_status()
}
```

Pre-allocate 5 rows for a 5-item max list. Call `sync_rows()` after every mutation.

### Numeric State Pattern

```splash
let count = 0
RoundedView{width:Fill height:Fit flow:Down spacing:10 padding:16 new_batch:true
  display := Label{text:"0" draw_text.color:#x44cc88 draw_text.text_style.font_size:32}
  View{flow:Right spacing:12 align:Align{x:0.5 y:0.5}
    ButtonFlat{text:"-" on_click:||{count -= 1; ui.display.set_text(count + "")}}
    ButtonFlat{text:"Reset" on_click:||{count = 0; ui.display.set_text("0")}}
    ButtonFlat{text:"+" on_click:||{count += 1; ui.display.set_text(count + "")}}
  }
}
```

Use `count + ""` to convert numbers to strings.

### Dynamic List Display

```splash
let task_count = 0
inp := TextInput{height:34}
lst := Label{text:"" font_size:14.0}
ButtonFlat{text:"Add" on_click:||{
  let t = ui.inp.text()
  if t != "" {
    task_count = task_count + 1
    let cur = ui.lst.text()
    if cur == " " { cur = "" }
    if cur != "" { cur = cur + "\n" }
    ui.lst.set_text(cur + task_count + ". " + t)
    ui.inp.set_text("")
  }
}}
```

### TextInput with on_return

```splash
input := TextInput{
    width: Fill height: 34
    empty_text: "Enter something"
    on_return: |text| add_item(text)
}
Button{text: "Add" width: 64 height: 34 on_click: || add_item(ui.input.text())}
```

### Sequential Digit Input

Perform arithmetic by accumulating digits:
```splash
let a = 0
ButtonFlat{text:"7" on_click:||{a = a*10+7; ui.display.set_text("" + a)}}
```

## Naming Children: `:=` vs `:`

Use `:=` for addressable children, `:` for static children:
```splash
label := Label{text:"default"}    // ✅ addressable via ui.label, overridable
label: Label{text:"default"}     // ❌ static — NOT addressable
```

Every path segment in an override must use `:=`:
```splash
// ✅ Correct
let Item = View{flow:Right
  texts := View{flow:Down
    label := Label{text:"default"}
  }
}
Item{texts.label.text:"new text"}  // works!

// ❌ Wrong — anonymous parent blocks override
let Item = View{flow:Right
  View{flow:Down
    label := Label{text:"default"}  // UNREACHABLE
  }
}
Item{label.text:"new text"}  // silent failure
```

## Styling Reference

| Property | Example | Effect |
|----------|---------|--------|
| `draw_bg.color` | `#x1e1e2e` | Background color (hex) |
| `draw_bg.border_radius` | `10.0` | Rounded corners |
| `draw_text.color` | `#xddd` | Text color |
| `draw_text.text_style.font_size` | `14` | Font size (float) |
| `padding` | `Inset{top:8 bottom:8 left:12 right:12}` | Inner padding |
| `spacing` | `10` | Gap between children in flow |
| `align` | `Align{x:0.5 y:0.5}` | Center alignment |
| `new_batch` | `true` | Required for text visibility on `show_bg:true` containers |
| `empty_text` | `"Type here..."` | Placeholder for TextInput |

## Not in Build

| Widget | Behavior |
|--------|----------|
| **`TabBar`** / **`Tab`** | width=0, height=0 — no visible output |

---

# Liquid Glass widgets (`glass.*`)

Makepad ships an Apple-style "Liquid Glass" widget kit under the `glass.` namespace. It is already in scope inside every `runsplash` block (no import needed). All names below are addressed as `glass.<Name>`.

> ## ⛔ HARD REQUIREMENT — use glass widgets
> The chat this renders in is a **glass-styled app**. Unless the user explicitly asks for a plain/non-glass look, you **MUST** build every app out of `glass.*` widgets on a detailed backdrop:
> - Surfaces/cards → `glass.Card` / `glass.Panel` / `glass.Group` (NOT plain `View`/`RoundedView` with a flat fill).
> - **Buttons → `glass.GlassButton` / `glass.GlassButtonProminent`** — these are the REAL liquid-glass buttons that lens and refract the backdrop (the proper "gauss glass" look). They are fully clickable (`on_click` works). **Prefer these for every button.** `glass.Button` / `glass.Chip` are flatter, non-refracting fallbacks — only use them if you specifically want a flat chip.
> - Toggles → `glass.GlassRadio` (a liquid-glass switch, `on_click` works).
> - Text → `glass.H1` / `glass.H2` / `glass.Body` / `glass.Caption` / `glass.OptionLabel`.
> - Inputs/toggles/sliders → `glass.TextInput` / `glass.SearchField` / `glass.Toggle` / `glass.Slider` / `glass.GlassRadio`.
> - And the app MUST open with a **detailed colourful backdrop** (see the golden rule below) so the glass is actually visible.
>
> An app that uses plain `Button`/`View`/`Label` on a flat dark fill is WRONG for this chat — it will look like a generic dark app with no glass.

## THE GOLDEN RULE: be TRANSPARENT — the chat already paints the backdrop

The chat this renders in **already paints a rich, detailed, colourful backdrop** behind your app. Your job is to float **transparent glass** on top of it so the glass refracts that backdrop. Glass lenses whatever is painted behind it; if you paint an opaque background, the glass has nothing to bend and the whole thing looks like a flat dark app.

**Therefore your app MUST be transparent:**

- **NEVER** put `show_bg: true` with an opaque `draw_bg.color` on your root or on any container. No `draw_bg.color: #x05070e`, no opaque `RoundedView`, no full-screen background layer, no `Svg` backdrop. The host backdrop is already there — let it show through.
- Build surfaces **only** from `glass.*` widgets (`glass.Card`, `glass.Panel`, `glass.Group`) — these are already translucent and refract the backdrop. Plain `View`/`RoundedView` with a fill will hide it.
- The root is just a transparent layout `View` (`flow: Down`, spacing, padding). Nothing else.

```splash
View{
  width: Fill height: Fit
  flow: Down spacing: 14 padding: 20
  // No background here — transparent on purpose so the chat backdrop shows through.
  glass.H1{text: "Glass Kit"}
  glass.Body{text: "A liquid-glass showcase."}
  glass.Card{
    glass.H2{text: "Liquid Glass"}
    glass.Body{text: "Floats above the chat backdrop, refracting it."}
  }
  View{width:Fill height:Fit flow:Right spacing:10
    glass.GlassButtonProminent{text:"Continue"}
    glass.GlassButton{text:"Cancel"}
  }
}
```

If you catch yourself writing `draw_bg.color` on a container, delete it and use a `glass.Card`/`glass.Panel` instead.

## Surfaces / containers

| Widget | Use for | Notes |
|--------|---------|-------|
| `glass.Panel` | Main glass card / sheet | Fill width, Fit height, flow Down, padding 16. Also aliased `GlassPanel`. |
| `glass.Card` | Content card | More tint than Panel — good default card. |
| `glass.Group` | Grouped controls | Tighter padding than Panel. |
| `glass.ClearPanel` | Clearer floating surface | Stronger lensing, less tint. |
| `glass.NavBar` | Top/bottom bar | height 58, flow Right. |
| `glass.TabBar` | Floating tab bar | height 66, flow Right. Put `glass.Caption`/`Label` children in it. (This is the glass TabBar — NOT the broken core `TabBar`.) |
| `glass.List` | List container | translucent panel, flow Down. |
| `glass.ListRow` | One row in a list | height 54, flow Right, vertically centered. |
| `glass.Badge` | Small status pill | green-tinted capsule. |

All surfaces already set `width`/`height`/`flow`/`padding`, so you can drop children straight in. They do NOT need `new_batch` (they manage their own batching), but the **outer dark root DOES** (`new_batch: true` + `show_bg: true`).

## Buttons (reliable — use these for clicks)

`glass.Button`, `glass.ProminentButton`, `glass.IconButton`, and `glass.Chip` are flat SDF glass buttons built on `ButtonFlat`. They are the **reliable interactive choice** and support `on_click`, `set_text()`, `text()` exactly like `Button`:

```splash
Row{flow:Right spacing:10
  glass.GlassButtonProminent{text:"Add" on_click:|| add_item(ui.inp.text())}
  glass.GlassButton{text:"Clear" on_click:|| clear_all()}
  glass.Chip{text:"All"}
}
```

## Premium lensing controls (decorative-first)

These are custom widgets that draw a real refracting lens over the scene. They look spectacular but are **best used as showcase / display controls** — prefer the flat `glass.Button` family when you need a reliable `on_click` handler in a generated app.

| Widget | What it is | Default size |
|--------|-----------|--------------|
| `glass.GlassButton` | Refracting pill button with a label | Fit × 44, has `text:` |
| `glass.GlassButtonProminent` | Blue-tinted refracting button | same |
| `glass.GlassRadio` | Liquid toggle switch (slides + "gloops") | 70 × 34 |
| `glass.GlassSlider` | Refracting slider knob | Fill × 32, `value: 0.4` |

```splash
ToggleRow{flow:Right spacing:16 align:Align{y:0.5}
  glass.GlassRadio{}
  glass.OptionLabel{text:"Wi-Fi"}
}
glass.GlassButton{text:"Continue"}
```

> Lensing widgets sample a blurred snapshot of the window. Inside the chat they fall back to a solid frosted color when no snapshot is available, so they still render — just less dramatically than in a full-screen app like `example-glass`.

## Inputs

| Widget | Use for |
|--------|---------|
| `glass.TextInput` | Glass text field (height 38) |
| `glass.SearchField` | Same, "Search" placeholder |
| `glass.Slider` | Labeled glass slider (height 40) |
| `glass.Toggle` | Glass on/off toggle |
| `glass.RadioButton` | Glass pill radio |

## Typography (pre-styled white-on-dark labels)

Use these instead of restyling `Label` by hand — they already set the right color and weight for dark glass UIs:

`glass.H1` (30), `glass.H2` (18), `glass.OptionLabel` (16), `glass.Body` (13, Fill width), `glass.ButtonLabel` (13), `glass.Caption` (11, muted blue — good for SECTION HEADERS).

## Full minimal glass app skeleton

```splash
fn noop(){}

RoundedView{
  width: Fill height: Fit
  flow: Down spacing: 14 padding: 20
  new_batch: true show_bg: true
  draw_bg.color: #x05070e
  draw_bg.border_radius: 18.0

  glass.H1{text: "Settings"}
  glass.Caption{text: "NETWORK"}
  glass.Card{
    View{width:Fill height:Fit flow:Right spacing:16 align:Align{y:0.5}
      glass.GlassRadio{}
      glass.OptionLabel{text:"Wi-Fi"}
    }
  }
  glass.Caption{text: "ACTIONS"}
  Row{flow:Right spacing:10
    glass.GlassButtonProminent{text:"Save" on_click:|| noop()}
    glass.GlassButton{text:"Cancel" on_click:|| noop()}
  }
}
```

## Interactivity that ACTUALLY works (read this for calculators / counters / forms)

The `ui` object is only injected into the scope of an **`on_click` / `on_return` / `on_change` closure**. It is **NOT visible inside a separately-declared `fn`**. So:

- ✅ Do all `ui.<id>.set_text(...)` / `ui.<id>.text()` calls **inline inside the closure**.
- ❌ Do NOT call a helper `fn` that itself references `ui` — it will fail with "variable ui not found".
- Shared state lives in top-level `let` variables (these persist across clicks). Mutate them inline.
- Arithmetic must stay **numeric** — never store a number as a string and add it (string `+` concatenates: `"5"+"3"=="53"`). Build numbers with `acc = acc*10 + digit`. Display with `"" + number`. Never use `as int` (produces NaN).
- The widget you update with `set_text` must be reachable as `ui.<id>` — give it a `:=` id and keep it directly addressable.

### Working glass calculator (numeric, inline `ui`, copy this shape)

```splash
let acc = 0.0       // number currently being entered
let stored = 0.0    // left-hand operand
let op = ""

View{ width: Fill height: Fit flow: Down spacing: 12 padding: 18
  // Transparent root — the chat backdrop shows through and the glass refracts it.
  glass.Card{ display := glass.H1{text:"0" width: Fill align: Align{x:1.0}} }
    View{width:Fill height:Fit flow:Right spacing:8
      glass.GlassButton{text:"7" width:Fill height:52 on_click:|| { acc = acc*10.0 + 7.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"8" width:Fill height:52 on_click:|| { acc = acc*10.0 + 8.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"9" width:Fill height:52 on_click:|| { acc = acc*10.0 + 9.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"+" width:Fill height:52 on_click:|| { stored = acc; acc = 0.0; op = "+" }}
    }
    View{width:Fill height:Fit flow:Right spacing:8
      glass.GlassButton{text:"4" width:Fill height:52 on_click:|| { acc = acc*10.0 + 4.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"5" width:Fill height:52 on_click:|| { acc = acc*10.0 + 5.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"6" width:Fill height:52 on_click:|| { acc = acc*10.0 + 6.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"-" width:Fill height:52 on_click:|| { stored = acc; acc = 0.0; op = "-" }}
    }
    View{width:Fill height:Fit flow:Right spacing:8
      glass.GlassButton{text:"1" width:Fill height:52 on_click:|| { acc = acc*10.0 + 1.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"2" width:Fill height:52 on_click:|| { acc = acc*10.0 + 2.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"3" width:Fill height:52 on_click:|| { acc = acc*10.0 + 3.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"*" width:Fill height:52 on_click:|| { stored = acc; acc = 0.0; op = "*" }}
    }
    View{width:Fill height:Fit flow:Right spacing:8
      glass.GlassButton{text:"0" width:Fill height:52 on_click:|| { acc = acc*10.0; ui.display.set_text("" + acc) }}
      glass.GlassButton{text:"C" width:Fill height:52 on_click:|| { acc = 0.0; stored = 0.0; op = ""; ui.display.set_text("0") }}
      glass.GlassButtonProminent{text:"=" width:Fill height:52 on_click:|| {
        let r = stored
        if op == "+" { r = stored + acc }
        if op == "-" { r = stored - acc }
        if op == "*" { r = stored * acc }
        acc = r; op = ""
        ui.display.set_text("" + r)
      }}
    }
  }
```

Every digit/operator handler is **inline** (no helper `fn`), so `ui.display` is in scope, and all arithmetic is numeric. This pattern computes correctly AND looks glassy.

## Glass gotchas

- **Transparent root is mandatory** — the chat paints the backdrop; if you paint your own opaque background the glass refracts nothing and looks like a flat dark app. No `draw_bg.color` / `show_bg` on your root or containers.
- For buttons use the **lensing** `glass.GlassButton` / `glass.GlassButtonProminent` (they refract the backdrop — the real gauss-glass look — and are fully clickable). For toggles use `glass.GlassRadio`. The flat `glass.Button`/`glass.Chip` don't refract; only use them when you explicitly want a flat chip.
- Don't restyle glass surface colors — they're tuned. Just set layout (`width`/`height`/`spacing`/`padding`) and drop content in.
- Hex colors with `e` next to a digit still need the `#x` prefix (e.g. `#x05070e`).