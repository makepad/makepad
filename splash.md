# Splash Script Manual (Terse AI Reference)

Splash is Makepad's UI scripting language. No commas between properties. No semicolons. Whitespace-delimited.

**Do NOT use `Root{}` or `Window{}`** — those are host-level wrappers handled externally. Your output is the content inside a body/splash widget.

---

## NAMING CHILDREN: Use `:=` for dynamic/list properties

In Splash, when you declare a named child widget inside a `let` template (or any container), you use the `:=` operator. This marks the child as a **named/dynamic** property — addressable and overridable per-instance.

- To declare: `label := Label{text: "default"}`
- To override: `MyTemplate{label.text: "new value"}`

If you write `label:` (colon) instead of `label :=` (colon-equals), the child is a **static** property — not addressable, and overrides fail silently (text becomes invisible).

**Use `:=` for any child you want to reference or override later:** `check :=`, `label :=`, `tag :=`, `title :=`, `body :=`, `icon :=`, `content :=`, etc.

## COPY-PASTE REFERENCE: Todo list

```
let TodoItem = View{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    check := CheckBox{text: ""}
    label := Label{text: "task" draw_text.color: #ddd draw_text.text_style.font_size: 11}
    Filler{}
    tag := Label{text: "" draw_text.color: #888 draw_text.text_style.font_size: 9}
}

RoundedView{
    width: 380 height: Fit
    flow: Down spacing: 4
    padding: 16
    new_batch: true
    draw_bg.color: #1e1e2e
    draw_bg.border_radius: 10.0
    Label{text: "My Tasks" draw_text.color: #fff draw_text.text_style.font_size: 14}
    Hr{}
    TodoItem{label.text: "Buy groceries" tag.text: "errands"}
    TodoItem{label.text: "Fix login bug" tag.text: "urgent"}
    TodoItem{label.text: "Write unit tests" tag.text: "dev"}
    TodoItem{label.text: "Call the dentist" tag.text: "personal"}
}
```

## COPY-PASTE REFERENCE: Card with title and body

```
let InfoCard = RoundedView{
    width: Fill height: Fit
    padding: 16 flow: Down spacing: 6
    draw_bg.color: #2a2a3d
    draw_bg.border_radius: 8.0
    title := Label{text: "Title" draw_text.color: #fff draw_text.text_style.font_size: 14}
    body := Label{text: "Body" draw_text.color: #aaa draw_text.text_style.font_size: 11}
}

View{
    flow: Down height: Fit spacing: 10 padding: 20
    InfoCard{title.text: "First card" body.text: "Some content here"}
    InfoCard{title.text: "Second card" body.text: "More content here"}
}
```

---

## 🚫 DO NOT INVENT SYNTAX OR PROPERTIES 🚫

**ONLY use widgets, properties, and syntax documented in this manual.** This code must compile and run — do not:

- Invent new properties (e.g., don't write `background_color:` — use `draw_bg.color:`)
- Guess at property names (e.g., don't write `font_size:` — use `draw_text.text_style.font_size:`)
- Make up new widgets that aren't listed here
- Suggest hypothetical features or syntax that "might work"
- Use CSS-like property names (no `border-radius`, use `draw_bg.border_radius`)

If you're unsure whether a property exists, **don't use it**. Stick to the exact syntax shown in the examples.

---

## 📝 OUTPUT FORMAT: CODE ONLY 📝

**When generating UI, output ONLY the Splash code.** Do not add:

- Explanatory text before or after the code
- "Here's the UI:" or "This creates..." preambles
- Suggestions for improvements or alternatives
- Commentary about what the code does

Just output the raw Splash script starting with `use mod.prelude.widgets.*` — nothing else.

---

## ⛔⛔⛔ CRITICAL: YOU MUST SET `height: Fit` ON EVERY CONTAINER ⛔⛔⛔

**STOP. READ THIS. THE #1 MISTAKE IS FORGETTING `height: Fit`.**

```
┌─────────────────────────────────────────────────────────────────┐
│  EVERY View, SolidView, RoundedView MUST HAVE height: Fit      │
│                                                                 │
│  ✅ View{ flow: Down height: Fit padding: 10 ... }              │
│                                                                 │
│  If you forget height: Fit, your UI will be INVISIBLE (0px)    │
└─────────────────────────────────────────────────────────────────┘
```

**Why?** The default is `height: Fill`. Your output renders in a `Fit` container. `Fill` inside `Fit` = circular dependency = **0 height**.

**ALWAYS write `height: Fit` immediately after the opening brace:**

```
View{ height: Fit flow: Down padding: 10
    Label{text: "Visible!"}
}

SolidView{ height: Fit width: Fill draw_bg.color: #333
    Label{text: "Also visible!"}
}

RoundedView{ height: Fit width: Fill flow: Down spacing: 8
    Label{text: "Card content"}
}
```

**Exceptions:**
1. Inside a fixed-height parent, `height: Fill` is OK:
```
View{ height: 300  // Fixed parent
    View{ height: Fill  // OK here - fills the 300px
        Label{text: "I fill the fixed 300px"}
    }
}
```
2. **MapView** — has no intrinsic height, so `height: Fit` also gives 0px. Use a **fixed pixel height**: `MapView{width: Fill height: 500}`

**TEMPLATE: Copy this pattern for every container:**
```
View{ height: Fit ...rest of properties...
    ...children...
}
```

---

## ⛔⛔⛔ CRITICAL: USE `width: Fill` ON THE ROOT CONTAINER ⛔⛔⛔

**NEVER use a fixed pixel width (e.g., `width: 400`) on your outermost container.** Your output renders inside a container that provides available width — use `width: Fill` to fill it.

```
┌─────────────────────────────────────────────────────────────────┐
│  The ROOT container MUST use  width: Fill                       │
│                                                                 │
│  ✅ RoundedView{ width: Fill height: Fit ... }                  │
│  ❌ RoundedView{ width: 400 height: Fit ... }                   │
│                                                                 │
│  Fixed widths make your UI a narrow sliver or completely broken │
└─────────────────────────────────────────────────────────────────┘
```

**Why?** A fixed width like `width: 400` does not adapt to the available space. Worse, if the parent container is narrower than 400, your content gets clipped. If a parse error occurs anywhere in the code, the entire layout can collapse to near-zero width.

**ALWAYS use `width: Fill` on the root element:**
```
RoundedView{ width: Fill height: Fit flow: Down
    // your content
}
```

Fixed pixel widths are fine for **inner elements** like icons, avatars, or specific components — just never on the outermost container.

---

## ⛔ CRITICAL: `draw_bg.border_radius` TAKES A FLOAT, NOT AN INSET ⛔

```
✅ draw_bg.border_radius: 16.0
❌ draw_bg.border_radius: Inset{top: 0 bottom: 16 left: 0 right: 0}
```

`border_radius` is a single `f32` value applied uniformly to all corners. Passing an `Inset` or object will cause a parse error that can **silently break your entire layout**.

---

## ⚠️ USE STYLED VIEWS, NOT RAW `View{}` ⚠️

**Do NOT use `View{ show_bg: true ... }`** — the raw View has an ugly green test color as its background.

Instead, use these pre-styled container widgets that have proper backgrounds:

| Widget | Use for |
|--------|---------|
| `SolidView` | Simple solid color background |
| `RoundedView` | Rounded corners with optional border |
| `RectView` | Rectangle with optional border |
| `RoundedShadowView` | Rounded corners with drop shadow |
| `RectShadowView` | Rectangle with drop shadow |
| `CircleView` | Circular shape |
| `GradientXView` | Horizontal gradient |
| `GradientYView` | Vertical gradient |

All have `show_bg: true` already set. Set color via `draw_bg.color`:

```
SolidView{ width: Fill height: Fit draw_bg.color: #334
    Label{text: "Content here"}
}

RoundedView{ width: Fill height: Fit draw_bg.color: #445 draw_bg.border_radius: 8.0
    Label{text: "Rounded card"}
}

RoundedShadowView{ width: Fill height: Fit draw_bg.color: #556 draw_bg.shadow_radius: 10.0
    Label{text: "Card with shadow"}
}
```

**Use raw `View{}` only when you need no background** (invisible layout container).

---

## Script Structure

Every splash script must start with a `use` statement to bring widgets into scope:

```
use mod.prelude.widgets.*

// Now all widgets (View, Label, Button, etc.) are available
View{
    flow: Down
    height: Fit  // ← ALWAYS set height! Default is Fill which breaks in Fit containers
    padding: 20
    Label{text: "Hello world"}
}
```

Without `use mod.prelude.widgets.*` at the top, widget names like `View`, `Label`, `Button` etc. will not be found.

### Let bindings for reusable definitions

Use `let` to define reusable widget templates. **`let` bindings must be defined ABOVE (before) the places where they are used.** They are local to the current scope.

**When a template has children you want to customize per-instance, you MUST use `id :=` declarations.** See the critical rule below.

```
use mod.prelude.widgets.*

// Simple template with NO per-instance children — just style overrides
let MyHeader = Label{
    draw_text.color: #fff
    draw_text.text_style.font_size: 16
}

// Template WITH per-instance children — MUST use id := declarations
let MyCard = RoundedView{
    width: Fill height: Fit
    padding: 15 flow: Down spacing: 8
    draw_bg.color: #334
    draw_bg.border_radius: 8.0
    title := Label{text: "default" draw_text.color: #fff draw_text.text_style.font_size: 16}
    body := Label{text: "" draw_text.color: #aaa}
}

// Override children using id.property syntax
View{
    flow: Down height: Fit
    spacing: 12 padding: 20
    MyCard{title.text: "First Card" body.text: "Content here"}
    MyCard{title.text: "Second Card" body.text: "More content"}
}
```

### Naming children in templates — the `:=` operator

Children inside a `let` template that you want to override per-instance MUST be declared with `:=`. This is part of the syntax — `label :=` creates a named/dynamic child, `label:` does not.

**Reusable todo/list item with multiple named children:**

```
let TodoItem = View{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 8
    align: Align{y: 0.5}
    check := CheckBox{text: ""}
    label := Label{text: "task" draw_text.color: #ddd draw_text.text_style.font_size: 11}
    Filler{}
    tag := Label{text: "" draw_text.color: #888 draw_text.text_style.font_size: 9}
}

View{
    flow: Down height: Fit spacing: 4
    TodoItem{label.text: "Walk the dog" tag.text: "personal"}
    TodoItem{label.text: "Fix login bug" tag.text: "urgent"}
    TodoItem{label.text: "Buy groceries" tag.text: "errands"}
}
```

You can override ANY property on an `id :=` child: `label.draw_text.color: #f00`, `icon.visible: false`, `subtitle.draw_text.text_style.font_size: 10`, etc.

**⛔ Named children inside anonymous containers are UNREACHABLE.** If a `:=` child is nested inside an anonymous `View{}` (no `:=` on the View), the override path cannot find it. The override fails silently and the default text shows instead:

Every container in the path from root to the child must have a `:=` name. Then use the full dot-path to override:
```
let Item = View{
    flow: Right
    texts := View{                           // named with :=
        flow: Down
        label := Label{text: "default"}
    }
}
Item{texts.label.text: "new text"}           // full path through named containers
```

## Syntax Fundamentals

```
// Property assignment
key: value

// Nested object
key: Type{ prop1: val1 prop2: val2 }

// Merge (extend parent, don't replace)
key +: { prop: val }

// Dot-path shorthand
draw_bg.color: #f00
// equivalent to: draw_bg +: { color: #f00 }

// Named child (:= declares a dynamic/addressable child)
my_button := Button{ text: "Click" }

// Anonymous child (no name)
Label{ text: "hello" }

// Let binding (define BEFORE use, local to current scope)
let MyThing = View{ height: Fit width: Fill }

// Instantiate let binding
MyThing{}

// Inherit from existing widget type
MyView = RoundedView{ height: Fit draw_bg.color: #f00 }
```

## Colors

```
#f00           // RGB short
#ff0000        // RGB full
#ff0000ff      // RGBA
#0000          // transparent black
vec4(1.0 0.0 0.0 1.0)  // explicit RGBA
```

## Sizing (Size enum)

```
width: Fill          // Fill available space (default)
width: Fit           // Shrink to content
width: 200           // Fixed 200px (bare number = Fixed)
width: Fill{min: 100 max: 500}
width: Fit{max: Abs(300)}
height: Fill height: Fit height: 100
```

## Layout

### Flow (direction children are laid out)
```
flow: Right          // default, left-to-right (no wrap)
flow: Down           // top-to-bottom
flow: Overlay        // stacked on top of each other
flow: Flow.Right{wrap: true}  // wrapping horizontal
flow: Flow.Down{wrap: true}   // wrapping vertical
```

### Spacing/Padding/Margin
```
spacing: 10                    // gap between children
padding: 15                    // uniform padding (bare number)
padding: Inset{top: 5 bottom: 5 left: 10 right: 10}
margin: Inset{top: 2 bottom: 2 left: 5 right: 5}
margin: 0.                    // uniform zero
```

### Alignment
```
align: Center                  // Align{x:0.5 y:0.5}
align: HCenter                 // Align{x:0.5 y:0.0}
align: VCenter                 // Align{x:0.0 y:0.5}
align: TopLeft                 // Align{x:0.0 y:0.0}
align: Align{x: 1.0 y: 0.0}   // top-right
align: Align{x: 0.0 y: 0.5}   // center-left
```

### Clipping
```
clip_x: true    // default
clip_y: true    // default
clip_x: false   // overflow visible
```

## View Widgets (containers)

All inherit from `ViewBase`. Default: no background.

| Widget | Background | Shape |
|--------|-----------|-------|
| `View` | none | - |
| `SolidView` | flat color | rectangle |
| `RoundedView` | color | rounded rect (`border_radius`) |
| `RoundedAllView` | color | per-corner radius (`vec4`) |
| `RoundedXView` | color | left/right radius (`vec2`) |
| `RoundedYView` | color | top/bottom radius (`vec2`) |
| `RectView` | color | rectangle with border |
| `RectShadowView` | color+shadow | rectangle |
| `RoundedShadowView` | color+shadow | rounded rect |
| `CircleView` | color | circle |
| `HexagonView` | color | hexagon |
| `GradientXView` | horizontal gradient | rectangle |
| `GradientYView` | vertical gradient | rectangle |
| `CachedView` | texture-cached | rectangle |
| `CachedRoundedView` | texture-cached | rounded rect |

### Scrollable Views
```
ScrollXYView{}     // scroll both axes
ScrollXView{}      // horizontal scroll
ScrollYView{}      // vertical scroll
```

### View Properties (all containers)
**⚠️ REMEMBER: Always set `height: Fit` (default is Fill which breaks in chat output!)**
```
// Layout (inherited by all containers)
width: Fill              // Size: Fill | Fit | <number>
height: Fit              // ⚠️ USE Fit! Default Fill breaks in Fit containers!
flow: Down               // Flow: Right | Down | Overlay | Flow.Right{wrap:true}
spacing: 10              // gap between children
padding: 15              // Inset or bare number
margin: 0.               // Inset or bare number
align: Center            // Align preset or Align{x: y:}

// Display
show_bg: true            // enable background drawing (false by default)
visible: true
new_batch: true              // see "Draw Batching" section below
cursor: MouseCursor.Hand
grab_key_focus: true
block_signal_event: false
capture_overload: false
clip_x: true
clip_y: true

// Scrollbar (for ScrollXView/ScrollYView/ScrollXYView)
scroll_bars: ScrollBar{}
```

### Draw Batching and `new_batch: true`

In Makepad, widgets that use the same shader are automatically collected into the same GPU draw call for performance. This means if you draw `Label{} SolidView{ Label{} }`, the second Label's text can end up **behind** the SolidView's background — because both Labels are batched into the same text draw call, which executes before the SolidView's background draw call.

**Set `new_batch: true` on any View that has `show_bg: true` AND contains text children.** This tells the View to start a new draw batch, ensuring its background is drawn before its children's text.

**⛔ CRITICAL for hover effects:** If a View has `show_bg: true` with a hover animator (background goes from transparent `#0000` to opaque on hover), you MUST set `new_batch: true` on that View. Without it, when the hover activates the background becomes opaque and covers the text — making text disappear on hover. This is the #1 mistake with hoverable list items.

**When to use `new_batch: true`:**
- **Any View/SolidView/RoundedView with `show_bg: true` that contains Labels or other text** — always add `new_batch: true`
- **Hoverable items** — a View with `show_bg: true` + animator hover that contains text MUST have `new_batch: true` or text vanishes on hover
- **Container of repeated items** that each have their own background — the container itself also needs `new_batch: true`
- When text appears invisible despite having the correct color — this is almost always a batching issue

```
// Hoverable item: new_batch ensures text draws on top of hover bg
let HoverItem = View{
    width: Fill height: Fit
    new_batch: true
    show_bg: true
    draw_bg +: { color: uniform(#0000) color_hover: uniform(#fff2) hover: instance(0.0) ... }
    animator: Animator{ hover: { ... } }
    label := Label{text: "item" draw_text.color: #fff}
}

// Parent container of repeated items also needs new_batch
RoundedView{
    flow: Down height: Fit new_batch: true
    HoverItem{label.text: "Walk the dog"}
    HoverItem{label.text: "Do laundry"}
}
```

### draw_bg Properties (for SolidView, RoundedView, etc.)
```
draw_bg +: {
    color: instance(#334)        // fill color
    color_2: instance(vec4(-1))  // gradient end (-1 = disabled)
    gradient_fill_horizontal: uniform(0.0)  // 0=vertical, 1=horizontal
    border_size: uniform(1.0)
    border_radius: uniform(5.0)  // for RoundedView
    border_color: instance(#888)
    border_inset: uniform(vec4(0))
    // Shadow views add:
    shadow_color: instance(#0007)
    shadow_radius: uniform(10.0)
    shadow_offset: uniform(vec2(0 0))
}
```

## Text Widgets

### Label
Properties: `text`, `draw_text` (DrawText), `align`, `flow`, `padding`, `hover_actions_enabled`

**⚠️ Label does NOT support `animator` or `cursor`.** Adding them has no effect — they are silently ignored. To make hoverable/clickable text, wrap a Label inside a `View` with animator+cursor (see Animator section for example).

```
Label{ text: "Hello" }
Label{
    width: Fit height: Fit
    draw_text.color: #fff
    draw_text.text_style.font_size: 12
    text: "Styled"
}
```

**⛔ CRITICAL: Default text color is WHITE.** All text widgets (Label, H1, H2, Button text, etc.) default to white (`#fff`). For light/white themes, you MUST explicitly set `draw_text.color` to a dark color on EVERY text element, or text will be invisible (white-on-white). Example:
For light themes, always set dark text explicitly:
```
RoundedView{ draw_bg.color: #f5f5f5 height: Fit new_batch: true
    Label{text: "Visible!" draw_text.color: #222}
}
```

### Label Variants
| Widget | Description |
|--------|-------------|
| `Label` | Default label |
| `Labelbold` | Bold font |
| `LabelGradientX` | Horizontal text gradient |
| `LabelGradientY` | Vertical text gradient |
| `TextBox` | Full-width, long-form text_style |
| `P` | Paragraph (like TextBox) |
| `Pbold` | Bold paragraph |

### Headings
```
H1{ text: "Title" }        // font_size_1
H2{ text: "Subtitle" }     // font_size_2
H3{ text: "Section" }      // font_size_3
H4{ text: "Subsection" }   // font_size_4
```

### draw_text Properties
```
draw_text +: {
    color: #fff
    color_2: uniform(vec4(-1))           // gradient end (-1 = disabled)
    color_dither: uniform(1.0)
    gradient_fill_horizontal: uniform(0.0)
    text_style: theme.font_regular{ font_size: 11 }
}
```
Available fonts: `theme.font_regular`, `theme.font_bold`, `theme.font_italic`, `theme.font_bold_italic`, `theme.font_code`, `theme.font_icons`

### TextInput
Properties: `is_password`, `is_read_only`, `is_numeric_only`, `empty_text`, `draw_bg`, `draw_text`, `draw_selection`, `draw_cursor`, `label_align`
```
TextInput{ width: Fill height: Fit empty_text: "Placeholder" }
TextInputFlat{ width: Fill height: Fit empty_text: "Type here" }
TextInput{ is_password: true empty_text: "Password" }
TextInput{ is_read_only: true }
TextInput{ is_numeric_only: true }
```

### LinkLabel
Properties: same as Button (text, draw_text, draw_bg, icon_walk, label_walk)
```
LinkLabel{ text: "Click me" }
```

### TextFlow (rich text container, used by Markdown/Html)
```
TextFlow{
    width: Fill height: Fit
    selectable: true
    font_size: 10
}
```

### Markdown / Html (feature-gated)
```
Markdown{
    width: Fill height: Fit
    selectable: true
    body: "# Title\n\nParagraph with **bold**"
}
Html{
    width: Fill height: Fit
    body: "<h3>Title</h3><p>Content</p>"
}
```

## Button Widgets

Properties: `text`, `draw_bg` (DrawQuad), `draw_text` (DrawText), `draw_icon` (DrawSvg), `icon_walk`, `label_walk`, `grab_key_focus`, `animator`

```
Button{ text: "Standard" }
ButtonFlat{ text: "Flat" }        // no bevel border
ButtonFlatter{ text: "Minimal" }  // invisible bg

// With icon
Button{
    text: "Save"
    icon_walk: Walk{width: 16 height: 16}
    draw_icon.color: #fff
    draw_icon.svg: crate_resource("self://path/to/icon.svg")
}

// Customize colors
ButtonFlat{
    text: "Custom"
    draw_bg +: {
        color: uniform(#336)
        color_hover: uniform(#449)
        color_down: uniform(#225)
    }
    draw_text +: {
        color: #fff
    }
}
```

### Button draw_bg Instance Variables
These are per-instance floats driven by the animator:
`hover`, `down`, `focus`, `disabled`

Color uniforms (each with `_hover`, `_down`, `_focus`, `_disabled` variants):
`color`, `color_2`, `border_color`, `border_color_2`

Other: `border_size`, `border_radius`, `color_dither`, `gradient_fill_horizontal`, `gradient_border_horizontal`

## Toggle Widgets

CheckBox/Toggle share a base. Properties: `text`, `draw_bg`, `draw_text`, `draw_icon`, `icon_walk`, `label_walk`, `label_align`, `animator`

```
CheckBox{ text: "Enable" }
CheckBoxFlat{ text: "Flat style" }
Toggle{ text: "Dark mode" }
ToggleFlat{ text: "Flat toggle" }
CheckBoxCustom{ text: "Custom" }
```

### CheckBox draw_bg Instance Variables
Animator-driven: `hover`, `down`, `focus`, `active`, `disabled`
Uniforms: `size`, `border_size`, `border_radius`
Color uniforms (each with `_hover`, `_down`, `_active`, `_focus`, `_disabled`): `color`, `border_color`, `mark_color`
Also: `mark_size`

### RadioButton
Properties: same as CheckBox
```
RadioButton{ text: "Option A" }
RadioButtonFlat{ text: "Option A" }
```

## Input Widgets

### Slider
Properties: `text`, `min`, `max`, `step`, `default`, `precision`, `axis` (DragAxis), `label_walk`, `label_align`, `draw_bg`, `draw_text`, `bind`
```
Slider{ width: Fill text: "Volume" min: 0.0 max: 100.0 default: 50.0 }
SliderMinimal{ text: "Value" min: 0.0 max: 1.0 step: 0.01 precision: 2 }
```

### DropDown
Properties: `labels` (string array), `draw_bg`, `draw_text`, `popup_menu`, `bind`, `bind_enum`
```
DropDown{ labels: ["Option A" "Option B" "Option C"] }
DropDownFlat{ labels: ["Small" "Medium" "Large"] }
```

## Media

### Image
Properties: `draw_bg` (DrawImage), `fit` (ImageFit), `min_width`, `min_height`, `width_scale`, `animation` (ImageAnimation)
```
Image{ width: 200 height: 150 fit: ImageFit.Stretch }
// ImageFit: Stretch | Horizontal | Vertical | Smallest | Biggest | Size
// ImageAnimation: Stop | Once | Loop | Bounce | OnceFps(60) | LoopFps(25) | BounceFps(25)
```

### DrawImage Properties
```
draw_bg +: {
    opacity: 1.0
    image_scale: vec2(1.0 1.0)
    image_pan: vec2(0.0 0.0)
    image_texture: texture_2d(float)
}
```

### Icon
Properties: `draw_bg`, `draw_icon` (DrawSvg), `icon_walk`
```
Icon{
    draw_icon.svg: crate_resource("self://resources/icons/my_icon.svg")
    draw_icon.color: #0ff
    icon_walk: Walk{width: 32 height: 32}
}
```

### LoadingSpinner
A View with animated arc shader. Properties: `color`, `rotation_speed`, `border_size`, `stroke_width`, `max_gap_ratio`, `min_gap_ratio`
```
LoadingSpinner{ width: 40 height: 40 }
```

## Layout Widgets

### Hr / Vr (dividers)
```
Hr{}     // horizontal rule
Vr{}     // vertical rule
```

### Filler (spacer)
```
Filler{}   // View{width: Fill height: Fill} - pushes siblings apart
```

**⛔ Do NOT use `Filler{}` next to a `width: Fill` sibling in `flow: Right`.** Both compete for remaining space and split it 50/50, causing text to be clipped halfway. Instead, give the content element `width: Fill` — it naturally pushes `width: Fit` siblings to the edge. Only use `Filler{}` between `width: Fit` siblings:
```
// Filler between Fit siblings — correct use
View{ flow: Right
    Label{text: "left"}
    Filler{}
    Label{text: "right"}
}

// width: Fill takes remaining space, pushes Fit siblings right — no Filler needed
View{ flow: Right
    texts := View{ width: Fill height: Fit flow: Down
        label := Label{text: "title"}
        sub := Label{text: "subtitle"}
    }
    tag := Label{text: "tag"}
}
```

### Splitter
Properties: `axis` (SplitterAxis), `align` (SplitterAlign), `a`, `b`, `size`, `min_horizontal`, `max_horizontal`, `min_vertical`, `max_vertical`, `draw_bg`
```
Splitter{
    axis: SplitterAxis.Horizontal   // Horizontal | Vertical
    align: SplitterAlign.FromA(250.0) // FromA(px) | FromB(px) | Weighted(0.5)
    a := left_panel
    b := right_panel
}
```
Note: `a` and `b` reference named children — use `a := left_panel` (the `:=` operator) to bind them.

### FoldHeader (collapsible section)
Properties: `body_walk`, `animator` (with `active` group: `on`/`off` states controlling `opened` float)
```
FoldHeader{
    header: View{ height: Fit
        flow: Right align: Align{y: 0.5} spacing: 8
        FoldButton{}
        Label{text: "Section Title"}
    }
    body: View{ height: Fit
        flow: Down padding: Inset{left: 23} spacing: 8
        // content
    }
}
```

## List Widgets

### PortalList (virtualized list)
Properties: `flow`, `scroll_bar`, `capture_overload`, `selectable`, `drag_scrolling`, `auto_tail`
Define templates with `:=` declarations. Templates are instantiated by host code at draw time.
```
list := PortalList{
    width: Fill height: Fill
    flow: Down
    scroll_bar: ScrollBar{}
    Item := View{
        width: Fill height: Fit
        title := Label{text: ""}
    }
    Header := View{ height: Fit ... }
}
```

### FlatList (non-virtualized)
```
FlatList{
    width: Fill height: Fill
    flow: Down
    Item := View{ height: Fit ... }
}
```

### ScrollBar
Properties: `bar_size`, `bar_side_margin`, `min_handle_size`, `draw_bg`
```
ScrollBar{
    bar_size: 10.0
    bar_side_margin: 3.0
    min_handle_size: 30.0
}
```

## Dock System

The Dock is a tabbed panel layout with splitters, tabs, and content templates. Three sections:
1. **`tab_bar +:`** — define tab header templates (appearance of tab buttons)
2. **`root :=`** — the layout tree of DockSplitter/DockTabs
3. **Content templates** — `Name := Widget{}` defines content instantiated by tabs

### Dock Properties
`tab_bar` (TabBar widget for tab headers), `splitter` (Splitter widget), `round_corner`, `drag_target_preview`, `padding`

### DockSplitter
`axis` (SplitterAxis), `align` (SplitterAlign), `a` (LiveId ref), `b` (LiveId ref)

### DockTabs
`tabs` (array of tab refs), `selected` (index), `closable`

### DockTab
`name` (string), `template` (ref to tab_bar template), `kind` (ref to content template)

### Complete Dock Example (from Makepad Studio)
```
Dock{
    width: Fill height: Fill

    // 1. Tab header templates (how tab buttons look)
    tab_bar +: {
        FilesTab := IconTab{
            draw_icon +: {
                color: #80FFBF
                svg: crate_resource("self://resources/icons/icon_file.svg")
            }
        }
        EditTab := IconTab{
            draw_icon +: {
                color: #FFB368
                svg: crate_resource("self://resources/icons/icon_editor.svg")
            }
        }
        LogTab := IconTab{
            draw_icon +: {
                color: #80FFBF
                svg: crate_resource("self://resources/icons/icon_log.svg")
            }
        }
    }

    // 2. Layout tree
    root := DockSplitter{
        axis: SplitterAxis.Horizontal
        align: SplitterAlign.FromA(250.0)
        a := left_tabs
        b := right_split
    }

    right_split := DockSplitter{
        axis: SplitterAxis.Vertical
        align: SplitterAlign.FromB(200.0)
        a := center_tabs
        b := bottom_tabs
    }

    left_tabs := DockTabs{
        tabs: [@files_tab]
        selected: 0
    }

    center_tabs := DockTabs{
        tabs: [@edit_tab]
        selected: 0
    }

    bottom_tabs := DockTabs{
        tabs: [@log_tab]
        selected: 0
    }

    // 3. Tab definitions (connect header template to content template)
    files_tab := DockTab{
        name: "Files"
        template := FilesTab        // references tab_bar template
        kind := FileTreeContent     // references content template
    }

    edit_tab := DockTab{
        name: "Editor"
        template := EditTab
        kind := EditorContent
    }

    log_tab := DockTab{
        name: "Log"
        template := LogTab
        kind := LogContent
    }

    // 4. Content templates (instantiated when tab is shown)
    FileTreeContent := View{
        flow: Down
        width: Fill height: Fill
        Label{text: "File tree here"}
    }

    EditorContent := View{
        flow: Down
        width: Fill height: Fill
        Label{text: "Editor here"}
    }

    LogContent := View{
        flow: Down
        width: Fill height: Fill
        Label{text: "Log here"}
    }
}
```

Dock variants: `Dock` (rounded corners), `DockFlat` (flat style)

## Navigation

### Modal
Properties: inherits View (flow: Overlay, align: Center). Contains `bg_view` (backdrop) and `content` (dialog body), both declared with `:=`.
```
my_modal := Modal{
    content +: {
        width: 300 height: Fit
        RoundedView{ height: Fit
            padding: 20 flow: Down spacing: 10
            draw_bg.color: #333
            Label{text: "Dialog Title"}
            close := ButtonFlat{text: "Close"}
        }
    }
}
```

### Tooltip
```
tooltip := Tooltip{}
```

### PopupNotification
```
popup := PopupNotification{
    align: Align{x: 1.0 y: 0.0}
    content +: { ... }
}
```

### SlidePanel
Properties: `side` (SlideSide), inherits View. Animated `active` float.
```
panel := SlidePanel{
    side: SlideSide.Left   // Left | Right | Top
    width: 200
    height: Fill
    // child content
}
```

### ExpandablePanel
Properties: `initial_offset`, inherits View (flow: Overlay). First child = background, `panel` (declared with `:=`) = draggable overlay.
```
ExpandablePanel{
    width: Fill height: Fill
    initial_offset: 100.0
    View{ height: Fit ... }          // background
    panel := View{ height: Fit ... }  // draggable panel
}
```

### PageFlip
Properties: `active_page` (LiveId), `lazy_init`. Children are page templates declared with `:=`.
```
PageFlip{
    active_page := page1
    page1 := View{ height: Fit ... }
    page2 := View{ height: Fit ... }
}
```

### StackNavigation
```
StackNavigation{
    root_view := View{ height: Fit ... }
    // StackNavigationViews added as children
}
```

### SlidesView
```
SlidesView{
    slide1 := Slide{
        title := H1{text: "Title"}
        SlideBody{text: "Content"}
    }
    slide2 := SlideChapter{
        title := H1{text: "Chapter"}
    }
}
```

### FileTree
```
FileTree{}
// Driven programmatically: begin_folder/end_folder/file
```

## Shader System

### Instance vs Uniform
```
draw_bg +: {
    hover: instance(0.0)      // per-draw-call, animatable by Animator
    color: uniform(#fff)       // shared across all instances of this shader variant
    tex: texture_2d(float)     // texture sampler
    my_var: varying(vec2(0))   // vertex→pixel interpolated (set in vertex shader)
}
```
**When to use each:**
- `instance()` — state that varies per widget (hover, down, focus, active, disabled), per-widget colors, scale/pan. Driven by the Animator system.
- `uniform()` — theme constants shared by all instances (border_size, border_radius, theme colors). Cannot be animated.

### Pixel Shader
```
draw_bg +: {
    pixel: fn() {
        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
        sdf.box(0. 0. self.rect_size.x self.rect_size.y 4.0)
        sdf.fill(#f00)
        return sdf.result  // already premultiplied by sdf.fill(), no Pal.premul() needed
    }
}
```

**⛔ CRITICAL: Premultiply colors returned from pixel()!** When you hand-code a `pixel: fn()` that returns a color (not via `sdf.result`), you MUST premultiply the alpha. Without this, colors with alpha (e.g. `#ffffff08`) will render as bright white instead of a subtle tint. Always wrap your return value in `Pal.premul()`:
```
pixel: fn(){
    return Pal.premul(self.color.mix(self.color_hover, self.hover))
}
```
Note: `sdf.fill()` / `sdf.stroke()` already premultiply internally, so `return sdf.result` is safe without extra `Pal.premul()`.

**Common pattern — fill + border stroke:**
```
pixel: fn() {
    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
    sdf.box(1. 1. self.rect_size.x - 2. self.rect_size.y - 2. 4.0)
    sdf.fill_keep(self.color)           // fill the shape, keep it for stroke
    sdf.stroke(self.border_color, 1.0)  // stroke the same shape's outline
    return sdf.result
}
```

### SDF Primitives
```
sdf.circle(cx cy radius)
sdf.rect(x y w h)
sdf.box(x y w h border_radius)
sdf.box_all(x y w h r_lt r_rt r_rb r_lb)   // per-corner radius
sdf.box_x(x y w h r_left r_right)
sdf.box_y(x y w h r_top r_bottom)
sdf.hexagon(cx cy radius)
sdf.hline(y half_height)
sdf.arc_round_caps(cx cy radius start_angle end_angle thickness)
sdf.arc_flat_caps(cx cy radius start_angle end_angle thickness)
```

### SDF Path Operations
```
sdf.move_to(x y)
sdf.line_to(x y)
sdf.close_path()
```

### SDF Combinators

These operate on the **current** shape and the **previous** shape. Draw two primitives, then combine:
```
sdf.union()        // merge shapes together (min of distances)
sdf.intersect()    // keep only overlap (max of distances)
sdf.subtract()     // cut current shape from previous shape
sdf.gloop(k)       // smooth/gooey union with rounding factor k
sdf.blend(k)       // linear blend: 0.0 = previous shape, 1.0 = current shape
```
Example — ring (circle with hole):
```
sdf.circle(cx cy outer_radius)
sdf.circle(cx cy inner_radius)
sdf.subtract()
sdf.fill(#fff)
```
Example — blend for toggle animation:
```
sdf.circle(x y r)         // ring shape (from subtract above)
sdf.circle(x y r)         // solid circle
sdf.blend(self.active)    // animate between ring (0) and solid (1)
```

### SDF Drawing
```
sdf.fill(color)           // fill and reset shape
sdf.fill_keep(color)      // fill, keep shape for subsequent stroke
sdf.stroke(color width)   // stroke and reset shape
sdf.stroke_keep(color w)  // stroke, keep shape
sdf.glow(color width)     // additive glow around shape, reset
sdf.glow_keep(color w)    // additive glow, keep shape
sdf.clear(color)          // clear result buffer with color
```

### SDF Transforms
```
sdf.translate(x y)
sdf.rotate(angle cx cy)
sdf.scale(factor cx cy)
```

### Built-in Shader Variables
```
self.pos              // vec2: normalized position [0,1] (computed from clipping in vertex shader)
self.rect_size        // vec2: pixel size of the drawn rect
self.rect_pos         // vec2: pixel position of the drawn rect
self.dpi_factor       // float: display DPI factor for high-DPI screens
self.draw_pass.time   // float: elapsed time in seconds (for continuous animation)
self.draw_pass.dpi_dilate  // float: DPI dilation factor for pixel-perfect strokes
self.draw_depth       // float: base depth for z-ordering
self.draw_zbias       // float: z-bias offset added to depth
self.geom_pos         // vec2: raw geometry position [0,1] (before clipping)
```

### Vertex Shader

Most widgets use the default vertex shader from DrawQuad. You can override it for custom geometry expansion (e.g., shadows) or DPI-aware texture coordinates:
```
draw_bg +: {
    // custom varying to pass data from vertex to pixel shader
    my_scale: varying(vec2(0))

    vertex: fn() {
        let dpi = self.dpi_factor
        let ceil_size = ceil(self.rect_size * dpi) / dpi
        self.my_scale = self.rect_size / ceil_size
        return self.clip_and_transform_vertex(self.rect_pos self.rect_size)
    }
    pixel: fn() {
        // my_scale is available here, interpolated from vertex
        return Pal.premul(self.color)
    }
}
```
`self.clip_and_transform_vertex(rect_pos rect_size)` is the standard helper that handles clipping, view shift (scrolling), and camera projection. Always call it in custom vertex shaders.

### Custom Shader Functions

You can define named functions on a draw shader for reuse:
```
draw_bg +: {
    get_color: fn() {
        return self.color
            .mix(self.color_hover, self.hover)
            .mix(self.color_down, self.down)
    }
    pixel: fn() {
        return Pal.premul(self.get_color())
    }
}
```
Functions with parameters:
```
draw_bg +: {
    get_color_at: fn(scale: vec2, pan: vec2) {
        return self.my_texture.sample(self.pos * scale + pan)
    }
}
```

### Mutable Variables

Use `let mut` to declare mutable variables in shader code:
```
pixel: fn() {
    let mut color = self.color
    if self.hover > 0.5 {
        color = self.color_hover
    }
    return Pal.premul(color)
}
```

### Conditionals and Match

Shaders support `if`/`else` and `match` on enum instance variables:
```
pixel: fn() {
    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
    if self.is_vertical > 0.5 {
        sdf.box(1. self.rect_size.y * self.scroll_pos 8. self.rect_size.y * self.handle_size 2.)
    } else {
        sdf.box(self.rect_size.x * self.scroll_pos 1. self.rect_size.x * self.handle_size 8. 2.)
    }
    sdf.fill(self.color)
    return sdf.result
}
```

### Texture Sampling

Declare texture samplers and sample them in pixel shaders:
```
draw_bg +: {
    my_tex: texture_2d(float)
    pixel: fn() {
        let color = self.my_tex.sample(self.pos)          // standard 2D sampling
        return Pal.premul(color)
    }
}
```
Alternative sampling functions:
```
sample2d(self.my_tex, uv)        // free-function form of texture sampling
sample2d_rt(self.image, uv)      // sample from render-target texture (handles Y-flip on some platforms)
```

### Color Operations
```
mix(color1 color2 factor)                   // linear interpolation (free function)
color1.mix(color2 factor)                   // method chaining form
#f00.mix(#0f0 0.5).mix(#00f hover)          // multi-chain for state interpolation
Pal.premul(color)                           // premultiply alpha — REQUIRED when returning from pixel()!
Pal.hsv2rgb(vec4(h s v 1.0))               // HSV to RGB conversion
Pal.rgb2hsv(color)                          // RGB to HSV conversion
Pal.iq(t a b c d)                           // Inigo Quilez cosine color palette
Pal.iq0(t) .. Pal.iq7(t)                   // pre-built cosine color palettes
```
⚠️ Always wrap your final color in `Pal.premul()` when returning from `pixel: fn()` (unless returning `sdf.result` which is already premultiplied).

**Gradient pattern** — use `vec4(-1.0, -1.0, -1.0, -1.0)` as a sentinel for "no gradient", then check with `if self.color_2.x > -0.5`:
```
color_2: uniform(vec4(-1.0, -1.0, -1.0, -1.0))   // sentinel: no gradient
pixel: fn() {
    let mut fill = self.color
    if self.color_2.x > -0.5 {
        let dither = Math.random_2d(self.pos.xy) * 0.04
        let dir = self.pos.y + dither
        fill = mix(self.color, self.color_2, dir)
    }
    return Pal.premul(fill)
}
```

### SDF Anti-aliasing

The `sdf.aa` field controls anti-aliasing sharpness. Default is computed from viewport. Set higher for sharper edges:
```
pixel: fn() {
    let sdf = Sdf2d.viewport(self.pos * self.rect_size)
    sdf.aa = sdf.aa * 3.0   // sharper edges (useful for small icons)
    sdf.move_to(c.x - sz, c.y - sz)
    sdf.line_to(c.x + sz, c.y + sz)
    sdf.stroke(#fff, 0.5 + 0.5 * self.draw_pass.dpi_dilate)
    return sdf.result
}
```

### SDF fill_premul / fill_keep_premul

When filling with a color that is already premultiplied (e.g., from a texture sample or render target):
```
sdf.fill_premul(color)           // fill with premultiplied color, reset shape
sdf.fill_keep_premul(color)      // fill with premultiplied color, keep shape
```

### GaussShadow (box shadows)
```
GaussShadow.box_shadow(lower upper point sigma)                    // fast rectangular shadow
GaussShadow.rounded_box_shadow(lower upper point sigma corner)     // rounded rectangle shadow
```
Used in shadow view variants (`RectShadowView`, `RoundedShadowView`) to render drop shadows efficiently.

### Math Utilities
```
// Custom Makepad functions
Math.random_2d(vec2)      // pseudo-random 0-1 from vec2 seed (for dithering)
Math.rotate_2d(v angle)   // 2D rotation of vector by angle

// Constants
PI                         // 3.14159...
E                          // 2.71828...
TORAD                      // degrees→radians multiplier (0.01745...)
GOLDEN                     // golden ratio (1.61803...)

// Standard GLSL math (all work on float, vec2, vec3, vec4)
sin(x) cos(x) tan(x) asin(x) acos(x) atan(y x)
pow(x y) sqrt(x) exp(x) exp2(x) log(x) log2(x)
abs(x) sign(x) floor(x) ceil(x) fract(x) mod(x y)
min(x y) max(x y) clamp(x min max)
step(edge x) smoothstep(edge0 edge1 x)

// Vector operations
length(v) distance(a b) dot(a b) cross(a b) normalize(v)

// Fragment-only (for advanced anti-aliasing)
dFdx(v) dFdy(v)           // partial derivatives (used in text SDF rendering)
```

## Animator

The animator drives `instance()` variables on draw shaders over time, enabling hover effects, transitions, and looping animations.

### ⛔ CRITICAL: Only Certain Widgets Support Animator ⛔

**NOT all widgets have an `animator` field.** If you add `animator: Animator{...}` to a widget that doesn't support it, the definition is **silently ignored** — no error, no hover, nothing happens.

**Widgets that SUPPORT animator:** `View`, `SolidView`, `RoundedView`, `ScrollXView`, `ScrollYView`, `ScrollXYView`, `Button`, `ButtonFlat`, `ButtonFlatter`, `CheckBox`, `Toggle`, `RadioButton`, `LinkLabel`, `TextInput`

**Widgets that DO NOT support animator:** `Label`, `H1`–`H4`, `P`, `TextBox`, `Image`, `Icon`, `Markdown`, `Html`, `Slider`, `DropDown`, `Splitter`, `Hr`, `Filler`

**To make a Label hoverable, wrap it in a View:**
```
View{
    width: Fill height: Fit
    cursor: MouseCursor.Hand
    show_bg: true
    draw_bg +: {
        color: uniform(#0000)
        color_hover: uniform(#fff2)
        hover: instance(0.0)
        pixel: fn(){
            return Pal.premul(self.color.mix(self.color_hover, self.hover))
        }
    }
    animator: Animator{
        hover: {
            default: @off
            off: AnimatorState{
                from: {all: Forward {duration: 0.15}}
                apply: {draw_bg: {hover: 0.0}}
            }
            on: AnimatorState{
                from: {all: Forward {duration: 0.15}}
                apply: {draw_bg: {hover: 1.0}}
            }
        }
    }
    Label{text: "hoverable item" draw_text.color: #fff}
}
```

### Structure

```
animator: Animator{
    <group_name>: {
        default: @<state_name>       // initial state (@ prefix required)
        <state_name>: AnimatorState{
            from: { ... }            // transition timing
            ease: <EaseFunction>     // optional ease override
            redraw: true             // optional: force redraw each frame
            apply: { ... }           // target values
        }
        <state_name>: AnimatorState{ ... }
    }
    <group_name>: { ... }           // multiple groups allowed
}
```

### Groups
Each group is an independent animation track (e.g. `hover`, `focus`, `active`, `disabled`, `time`). Multiple groups animate simultaneously without interfering.

### The `from` Block
Controls when/how the transition plays. Keys are state names being transitioned FROM, or `all` as catch-all:
```
from: {all: Forward {duration: 0.2}}           // from any state
from: {all: Snap}                               // instant from any state
from: {
    all: Forward {duration: 0.1}                // default
    down: Forward {duration: 0.01}              // faster when coming from "down"
}
```

### The `apply` Block
Target values to animate TO. The structure mirrors the widget's property tree. Keys are the widget's sub-objects (like `draw_bg`, `draw_text`), values are the shader instance variables to animate:

```
apply: {
    draw_bg: {hover: 1.0}                      // animate draw_bg.hover to 1.0
    draw_text: {hover: 1.0}                     // animate draw_text.hover to 1.0
}
```

Multiple properties in one block:
```
apply: {
    draw_bg: {down: 1.0, hover: 0.5}
    draw_text: {down: 1.0, hover: 0.5}
}
```

For non-draw properties (e.g. a float field on the widget itself):
```
apply: {
    opened: 1.0                                 // animate widget's own "opened" field
    active: 0.0                                 // animate widget's own "active" field
}
```

### snap() — Instant Jump
Wrapping a value in `snap()` makes it jump instantly instead of interpolating:
```
apply: {
    draw_bg: {down: snap(1.0), hover: 1.0}     // down jumps, hover interpolates
}
```

### timeline() — Keyframes
Animate through multiple values over the duration using time/value pairs (times 0.0–1.0):
```
apply: {
    draw_bg: {anim_time: timeline(0.0 0.0  1.0 1.0)}   // linear 0→1
}
```

### Complete Button Animator Example
```
animator: Animator{
    disabled: {
        default: @off
        off: AnimatorState{
            from: {all: Forward {duration: 0.}}
            apply: {
                draw_bg: {disabled: 0.0}
                draw_text: {disabled: 0.0}
            }
        }
        on: AnimatorState{
            from: {all: Forward {duration: 0.2}}
            apply: {
                draw_bg: {disabled: 1.0}
                draw_text: {disabled: 1.0}
            }
        }
    }
    hover: {
        default: @off
        off: AnimatorState{
            from: {all: Forward {duration: 0.1}}
            apply: {
                draw_bg: {down: 0.0, hover: 0.0}
                draw_text: {down: 0.0, hover: 0.0}
            }
        }
        on: AnimatorState{
            from: {
                all: Forward {duration: 0.1}
                down: Forward {duration: 0.01}
            }
            apply: {
                draw_bg: {down: 0.0, hover: snap(1.0)}
                draw_text: {down: 0.0, hover: snap(1.0)}
            }
        }
        down: AnimatorState{
            from: {all: Forward {duration: 0.2}}
            apply: {
                draw_bg: {down: snap(1.0), hover: 1.0}
                draw_text: {down: snap(1.0), hover: 1.0}
            }
        }
    }
    focus: {
        default: @off
        off: AnimatorState{
            from: {all: Snap}
            apply: {
                draw_bg: {focus: 0.0}
                draw_text: {focus: 0.0}
            }
        }
        on: AnimatorState{
            from: {all: Snap}
            apply: {
                draw_bg: {focus: 1.0}
                draw_text: {focus: 1.0}
            }
        }
    }
    time: {
        default: @off
        off: AnimatorState{
            from: {all: Forward {duration: 0.}}
            apply: {}
        }
        on: AnimatorState{
            from: {all: Loop {duration: 1.0, end: 1000000000.0}}
            apply: {
                draw_bg: {anim_time: timeline(0.0 0.0  1.0 1.0)}
            }
        }
    }
}
```

### Play Types (transition modes)
```
Forward {duration: 0.2}                        // play once forward
Snap                                            // instant (no interpolation)
Reverse {duration: 0.2, end: 1.0}             // play in reverse
Loop {duration: 1.0, end: 1000000000.0}        // repeat forward
ReverseLoop {duration: 1.0, end: 1.0}         // repeat in reverse
BounceLoop {duration: 1.0, end: 1.0}          // bounce back and forth
```

### Ease Functions
```
Linear                  // default
InQuad  OutQuad  InOutQuad
InCubic OutCubic InOutCubic
InQuart OutQuart InOutQuart
InQuint OutQuint InOutQuint
InSine  OutSine  InOutSine
InExp   OutExp   InOutExp
InCirc  OutCirc  InOutCirc
InElastic  OutElastic  InOutElastic
InBack     OutBack     InOutBack
InBounce   OutBounce   InOutBounce
ExpDecay {d1: 0.82, d2: 0.97, max: 100}
Pow {begin: 0.0, end: 1.0}
Bezier {cp0: 0.0, cp1: 0.0, cp2: 1.0, cp3: 1.0}
```

## Theme Variables (prefix: `theme.`)

### Spacing
`space_1` `space_2` `space_3`

### Inset Presets
`mspace_1` `mspace_2` `mspace_3` (uniform)
`mspace_h_1` `mspace_h_2` `mspace_h_3` (horizontal only)
`mspace_v_1` `mspace_v_2` `mspace_v_3` (vertical only)

### Dimensions
`corner_radius` `beveling` `tab_height` `splitter_size` `container_corner_radius` `dock_border_size`

### Colors (key ones)
`color_bg_app` `color_fg_app` `color_bg_container` `color_bg_even` `color_bg_odd`
`color_text` `color_text_hl` `color_text_disabled`
`color_label_inner` `color_label_outer` (+ `_hover` `_down` `_focus` `_active` `_disabled`)
`color_inset` (+ variants) `color_outset` (+ variants)
`color_bevel` (+ variants)
`color_shadow` `color_highlight` `color_makepad` (#FF5C39)
`color_white` `color_black`
`color_error` `color_warning` `color_panic`
`color_selection_focus` `color_cursor`
`color_u_1`..`color_u_6` (light scale) `color_d_1`..`color_d_5` (dark scale)
`color_u_hidden` `color_d_hidden` (transparent)
`color_drag_target_preview`
`color_val` `color_handle` (+ `_hover` `_focus` `_drag` `_disabled`) — slider colors
`color_mark_off` `color_mark_active` (+ variants) — check/radio marks
`color_app_caption_bar`

### Typography
`font_size_1`..`font_size_4` `font_size_p` `font_size_code` `font_size_base`
`font_regular` `font_bold` `font_italic` `font_bold_italic` `font_code` `font_icons`
`font_wdgt_line_spacing` `font_longform_line_spacing`

## Enums Reference

### MouseCursor
`Default` `Hand` `Arrow` `Text` `Move` `Wait` `Help` `NotAllowed` `Crosshair` `Grab` `Grabbing` `NResize` `EResize` `SResize` `WResize` `NsResize` `EwResize` `ColResize` `RowResize` `Hidden`
Usage: `cursor: MouseCursor.Hand`

### ImageFit
`Stretch` `Horizontal` `Vertical` `Smallest` `Biggest` `Size`

### SplitterAxis
`Horizontal` `Vertical`

### SplitterAlign
`FromA(250.0)` `FromB(200.0)` `Weighted(0.5)`

### SlideSide
`Left` `Right` `Top`

### DragAxis (for Slider)
`Horizontal` `Vertical`

### ImageAnimation
`Stop` `Once` `Loop` `Bounce` `Frame(0.0)` `Factor(0.0)` `OnceFps(60.0)` `LoopFps(60.0)` `BounceFps(60.0)`

## Common Patterns

**REMINDER: Every container below uses `height: Fit` — you must too!**

### Colored card
```
RoundedView{
    width: Fill height: Fit
    padding: 15 flow: Down spacing: 8
    draw_bg.color: #445
    draw_bg.border_radius: 8.0
    Label{text: "Card Title" draw_text.color: #fff}
}
```

### Sidebar + content
```
View{
    width: Fill height: Fill
    flow: Right
    SolidView{
        width: 250 height: Fill
        draw_bg.color: #222
        flow: Down padding: 10
    }
    View{
        width: Fill height: Fill
        flow: Down padding: 15
    }
}
```

### Sidebar + content using Splitter
```
Splitter{
    axis: SplitterAxis.Horizontal
    align: SplitterAlign.FromA(250.0)
    a := sidebar
    b := main
}
sidebar := View{ width: Fill height: Fill flow: Down padding: 10 }
main := View{ width: Fill height: Fill flow: Down padding: 15 }
```

### Overlay (modal/tooltip pattern)
```
View{ height: Fit
    flow: Overlay
    View{ height: Fit width: Fill ... }   // base content
    View{ height: Fit align: Center ... } // overlay on top
}
```

### Scrollable list
```
ScrollYView{
    width: Fill height: Fill
    flow: Down padding: 10 spacing: 8
    Label{text: "Item 1"}
    Label{text: "Item 2"}
}
```

### Custom shader widget
Note: `View{ show_bg: true }` is OK here because we provide a complete custom `pixel` shader that overrides the ugly default.
```
View{
    width: 200 height: 200
    show_bg: true
    draw_bg +: {
        pixel: fn(){
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(
                self.rect_size.x * 0.5
                self.rect_size.y * 0.5
                min(self.rect_size.x self.rect_size.y) * 0.4
            )
            sdf.fill(#f80)
            return sdf.result  // already premultiplied by sdf.fill(), no Pal.premul() needed
        }
    }
}
```

### Hoverable list item
Label does NOT support animator. Wrap it in a View to get hover effects. Use `label :=` to declare the inner Label so each instance can override its text via `label.text:`:
```
let HoverItem = View{
    width: Fill height: Fit
    padding: 8
    cursor: MouseCursor.Hand
    new_batch: true
    show_bg: true
    draw_bg +: {
        color: uniform(#0000)
        color_hover: uniform(#fff2)
        hover: instance(0.0)
        pixel: fn(){
            return self.color.mix(self.color_hover, self.hover)
        }
    }
    animator: Animator{
        hover: {
            default: @off
            off: AnimatorState{
                from: {all: Forward {duration: 0.15}}
                apply: {draw_bg: {hover: 0.0}}
            }
            on: AnimatorState{
                from: {all: Forward {duration: 0.15}}
                apply: {draw_bg: {hover: 1.0}}
            }
        }
    }
    label := Label{text: "item" draw_text.color: #fff}
}

RoundedView{
    width: 300 height: Fit
    padding: 10 flow: Down spacing: 4
    new_batch: true
    draw_bg.color: #222
    draw_bg.border_radius: 5.0
    Label{text: "Todo Items" draw_text.color: #fff}
    HoverItem{label.text: "Walk the dog"}
    HoverItem{label.text: "Do laundry"}
    HoverItem{label.text: "Buy groceries"}
}
```

### Toolbar pattern
```
RectShadowView{
    width: Fill height: 38.
    flow: Down padding: theme.mspace_2
    draw_bg +: {
        shadow_color: theme.color_shadow
        shadow_radius: 7.5
        color: theme.color_fg_app
    }
    content := View{
        height: Fit width: Fill
        flow: Right spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
        ButtonFlatter{text: "File"}
        ButtonFlatter{text: "Edit"}
        Filler{}
        ButtonFlat{text: "Run"}
    }
}
```

## HTTP Requests (`net.http_request`)

Make async HTTP requests from script. Responses arrive via callbacks.

### GET request
```
let req = net.HttpRequest{
    url: "https://html.duckduckgo.com/html/?q=rust+programming"
    method: net.HttpMethod.GET
    headers: {"User-Agent": "MakepadApp/1.0"}
}
net.http_request(req) do net.HttpEvents{
    on_response: |res| {
        let text = res.body.to_string()       // body as string
        let json = res.body.parse_json()      // or parse as JSON
        // res.status_code                    // HTTP status (200, 404, etc.)
    }
    on_error: |e| {
        // e.message                          // error description
    }
}
```

### POST request with JSON body
```
let req = net.HttpRequest{
    url: "https://api.example.com/data"
    method: net.HttpMethod.POST
    headers: {"Content-Type": "application/json"}
    body: {key: "value" count: 42}.to_json()
}
net.http_request(req) do net.HttpEvents{
    on_response: |res| { /* ... */ }
    on_error: |e| { /* ... */ }
}
```

### Streaming response
```
let req = net.HttpRequest{
    url: "https://api.example.com/stream"
    method: net.HttpMethod.POST
    is_streaming: true
    body: {stream: true}.to_json()
}
var total = ""
net.http_request(req) do net.HttpEvents{
    on_stream: |res| {
        total += res.body.to_string()         // called per chunk
    }
    on_complete: |res| {
        // stream finished, total has all data
    }
    on_error: |e| { /* ... */ }
}
```

### HttpMethod values
`net.HttpMethod.GET`, `POST`, `PUT`, `DELETE`, `HEAD`, `PATCH`, `OPTIONS`

### Cookie-free search endpoints
DuckDuckGo provides HTML endpoints that return static HTML — no cookies, no JS, no API key:
- `https://html.duckduckgo.com/html/?q=QUERY` — div-based, CSS classes for results
- `https://lite.duckduckgo.com/lite/?q=QUERY` — table-based, ~10kB compressed

Both require a `User-Agent` header. Results can be parsed with `parse_html()`.

---

## HTML Parsing (`parse_html`)

Parse an HTML string and query it with CSS-like selectors. Call `.parse_html()` on any string.

### Basic usage
```
let html = "<div class='box' id='main'><p>Hello</p><p class='bold'>World</p></div>"
let doc = html.parse_html()
```

### Querying elements
```
doc.query("p")                // all <p> elements (returns html handle)
doc.query("p[0]")             // first <p> element
doc.query("#main")            // element with id "main"
doc.query("p.bold")           // <p> with class "bold"
doc.query("div > p")          // direct children
doc.query("div p")            // descendants
doc.query("div > *")          // all direct children (wildcard)
doc.query("div").query("p")   // chained queries
```

### Extracting data
```
doc.query("p[0]").text         // text content: "Hello"
doc.query("div@class")        // attribute value: "box"
doc.query("div@id")           // attribute value: "main"
doc.query("p.text")           // array of text from all <p>: ["Hello", "World"]
doc.query("p@class")          // array of class attrs from all <p>
```

### Properties on html handles
```
handle.length                  // number of matched elements
handle.text                    // text content (concatenated)
handle.html                    // reconstructed HTML string
handle.attr("name")            // attribute value (string or nil)
handle.array()                 // convert to array of element handles
```

### Iterating results
```
let items = doc.query("a.result__a").array()
for item, i in items {
    let title = item.text
    let href = item.attr("href")
    // ... use title and href
}
```

### Full example: search DuckDuckGo and parse results
```
fn do_search(query) {
    let req = net.HttpRequest{
        url: "https://html.duckduckgo.com/html/?q=" + query
        method: net.HttpMethod.GET
        headers: {"User-Agent": "MakepadApp/1.0"}
    }
    net.http_request(req) do net.HttpEvents{
        on_response: |res| {
            let doc = res.body.to_string().parse_html()
            let links = doc.query("a.result__a").array()
            let snippets = doc.query("a.result__snippet").array()
            for link, i in links {
                let title = link.text
                let url = link.attr("href")
                let snippet = if i < snippets.len() snippets[i].text else ""
                // ... build result list
            }
        }
        on_error: |e| { /* handle error */ }
    }
}
```

---

## Notes

- **⛔ Default text color is WHITE.** For light/white themes, set `draw_text.color` to a dark color (e.g. `#222`, `#333`) on ALL text elements. Otherwise text is invisible (white-on-white).
- **⛔ Set `new_batch: true` on ANY View with `show_bg: true` that contains text.** Makepad batches same-shader widgets into one draw call. Without `new_batch: true`, text renders behind backgrounds (invisible text). This is especially critical for **hoverable items** — text vanishes on hover when the background becomes opaque. Set it on BOTH the item template AND the parent container.
- **⚠️ ALWAYS set `height: Fit` on containers!** The default is `height: Fill` which causes 0-height (invisible UI) in this context.
- **⛔ Named children in `let` templates MUST use `:=`:** `label := Label{...}`, `tag := Label{...}`, `check := CheckBox{...}`. Override with `Item{label.text: "x"}`. Without `:=`, text is invisible.
- **⛔ Named children inside anonymous Views are UNREACHABLE.** If `label :=` is inside an unnamed `View{}`, `Item{label.text: "x"}` fails silently. Give the View a name: `texts := View{ label := Label{...} }` then override with `Item{texts.label.text: "x"}`.
- **🚫 DO NOT invent properties or syntax.** Only use what's documented in this manual. No guessing.
- No commas between sibling properties (space or newline separated)
- **Use commas when values contain negative numbers or could be parsed as expressions**: `vec4(-1.0, -1.0, -1.0, -1.0)` NOT `vec4(-1.0 -1.0 -1.0 -1.0)` (the parser would see `-1.0 -1.0` as subtraction). Safe rule: always use commas inside `vec2()`, `vec4()`, and array literals when any value is negative or an expression
- `+:` merges with parent; without it, replaces entirely
- `:=` declares named/dynamic/template children (e.g. `label := Label{...}`)
- Bare numbers for Size become `Fixed(n)`: `width: 200` = `width: Size.Fixed(200)`
- Resources: `crate_resource("self://relative/path")`
- Function args in shaders: space-separated, no commas: `sdf.box(0. 0. 100. 100. 5.0)`
- `if` in shaders: `if condition { ... } else { ... }` (no parens around condition)
- `for` in shaders: `for i in 0..4 { ... }`
- `match` in shaders: `match self.block_type { Type.A => { ... } Type.B => { ... } }`
- Inherit + override: `theme.mspace_1{left: theme.space_2}` — takes mspace_1 but overrides left
- Strings use double quotes only: `text: "Hello"`. No single quotes, no backticks.

## Guidelines

- Use runsplash blocks for anything visual: UI mockups, styled cards, layouts, color palettes, shader demos, button groups, form layouts, etc.
- You can have multiple runsplash blocks in a single response, mixed with normal markdown text.
- Keep splash blocks focused — one concept per block when possible.
- Use `let` bindings at the top of a block to define reusable styled components, then instantiate them below.
- Use theme variables (theme.color_bg_app, theme.space_2, etc.) for consistent styling.
- For simple text answers, just use normal markdown without runsplash blocks.

## Vector Widget (SVG-like Drawing)

The `Vector{}` widget renders SVG-like vector graphics declaratively in Splash. It supports paths, shapes, gradients, filters, groups, transforms, and animations — all without loading external SVG files.

### Basic Usage

```
Vector{width: 200 height: 200 viewbox: vec4(0 0 200 200)
    Rect{x: 10 y: 10 w: 80 h: 60 rx: 5 ry: 5 fill: #f80}
    Circle{cx: 150 cy: 50 r: 30 fill: #08f}
    Line{x1: 10 y1: 150 x2: 190 y2: 150 stroke: #fff stroke_width: 2}
}
```

The `viewbox` property defines the coordinate space as `vec4(x y width height)`. The widget sizes itself to fit the viewbox when `width: Fit` and `height: Fit` (the defaults), or you can set explicit pixel dimensions.

### Shape Types

All shapes support these common style properties:

| Property | Type | Default | Notes |
|----------|------|---------|-------|
| `fill` | color, Gradient, RadGradient, Tween, or `false` | inherited | `false` = no fill |
| `fill_opacity` | f32 | 1.0 | multiplied with fill alpha |
| `stroke` | color, Gradient, or Tween | none | outline color |
| `stroke_width` | f32 or Tween | 0.0 | outline thickness |
| `stroke_opacity` | f32 or Tween | 1.0 | outline alpha |
| `opacity` | f32 or Tween | 1.0 | overall shape opacity |
| `stroke_linecap` | string | "butt" | "butt", "round", "square" |
| `stroke_linejoin` | string | "miter" | "miter", "round", "bevel" |
| `transform` | Transform or array | identity | see Transforms section |
| `filter` | Filter ref | none | see Filters section |
| `shader_id` | f32 | 0.0 | for custom GPU effects on Svg widget |

#### Path — SVG path data
```
Path{d: "M 10 10 L 100 100 C 50 50 200 200 300 300 Z" fill: #f00 stroke: #000 stroke_width: 2}
```
The `d` property accepts standard SVG path data strings (M, L, C, Q, A, Z, etc.).

#### Rect — Rectangle
```
Rect{x: 10 y: 20 w: 100 h: 50 rx: 5 ry: 5 fill: #f80 stroke: #fff stroke_width: 1}
```

#### Circle
```
Circle{cx: 50 cy: 50 r: 40 fill: #08f}
```

#### Ellipse
```
Ellipse{cx: 100 cy: 50 rx: 80 ry: 40 fill: #0f8}
```

#### Line
```
Line{x1: 10 y1: 10 x2: 190 y2: 190 stroke: #fff stroke_width: 2 stroke_linecap: "round"}
```

#### Polyline — open connected segments
```
Polyline{pts: [10 10 50 80 100 20 150 90] fill: false stroke: #ff0 stroke_width: 2}
```

#### Polygon — closed connected segments
```
Polygon{pts: [100 10 40 198 190 78 10 78 160 198] fill: #f0f stroke: #fff stroke_width: 1}
```

### Groups

`Group{}` composes shapes and applies shared styles/transforms to all children:

```
Vector{width: 200 height: 200 viewbox: vec4(0 0 200 200)
    Group{opacity: 0.7 transform: Rotate{deg: 15}
        Rect{x: 20 y: 20 w: 60 h: 60 fill: #f00}
        Circle{cx: 130 cy: 50 r: 30 fill: #0f0}
    }
}
```

Groups can be nested. Style properties on a Group (fill, stroke, etc.) apply to its children.

### Gradients

Define gradients as `let` bindings and reference them in `fill` or `stroke`:

#### Linear Gradient
```
let my_grad = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
    Stop{offset: 0 color: #ff0000}
    Stop{offset: 0.5 color: #00ff00}
    Stop{offset: 1 color: #0000ff}
}

Vector{width: 200 height: 100 viewbox: vec4(0 0 200 100)
    Rect{x: 0 y: 0 w: 200 h: 100 fill: my_grad}
}
```

Gradient coordinates (`x1`, `y1`, `x2`, `y2`) are in the range 0–1 (object bounding box). `Stop` children define color stops with `offset` (0–1), `color`, and optional `opacity`.

#### Radial Gradient
```
let radial = RadGradient{cx: 0.5 cy: 0.5 r: 0.5
    Stop{offset: 0 color: #fff}
    Stop{offset: 1 color: #000}
}

Vector{width: 200 height: 200 viewbox: vec4(0 0 200 200)
    Circle{cx: 100 cy: 100 r: 90 fill: radial}
}
```

RadGradient properties: `cx`, `cy` (center, default 0.5), `r` (radius, default 0.5), `fx`, `fy` (focal point, defaults to center).

#### Gradient stops with opacity
```
let glass = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
    Stop{offset: 0 color: #xffffff opacity: 0.35}
    Stop{offset: 0.4 color: #xffffff opacity: 0.08}
    Stop{offset: 1 color: #xffffff opacity: 0.2}
}
```

### Filters

Define a `Filter` with `DropShadow` effects:

```
let shadow = Filter{
    DropShadow{dx: 2 dy: 4 blur: 6 color: #000000 opacity: 0.5}
}

Vector{width: 200 height: 200 viewbox: vec4(0 0 200 200)
    Rect{x: 40 y: 40 w: 120 h: 120 rx: 10 ry: 10 fill: #445 filter: shadow}
}
```

DropShadow properties: `dx` (x offset), `dy` (y offset), `blur` (blur radius), `color`, `opacity`.

### Transforms

Transforms can be applied to any shape or group via the `transform` property. Use a single transform or an array of transforms (composed left-to-right):

#### Static transforms
```
// Single transform
Rect{x: 0 y: 0 w: 50 h: 50 fill: #f00 transform: Rotate{deg: 45}}

// Multiple transforms (composed left-to-right)
Group{transform: [Translate{x: 100 y: 50} Scale{x: 2 y: 2} Rotate{deg: 30}]
    Circle{cx: 0 cy: 0 r: 20 fill: #0ff}
}
```

Available transforms:
- `Rotate{deg: 45}` — rotation in degrees. Optional `cx`, `cy` for rotation center
- `Scale{x: 2 y: 1.5}` — scale factors. If only `x` is given, `y` defaults to the same value
- `Translate{x: 100 y: 50}` — translation offset
- `SkewX{deg: 30}` — horizontal skew
- `SkewY{deg: 15}` — vertical skew

#### Animated transforms
Add `dur`, `from`, `to` (or `values`), and optionally `loop_` and `begin` to animate:

```
// Continuously rotating shape
Circle{cx: 100 cy: 100 r: 30 fill: #0ff
    transform: Rotate{deg: 0 dur: 2.0 from: 0 to: 360 loop_: true}
}

// Animated scale
Rect{x: 50 y: 50 w: 40 h: 40 fill: #f80
    transform: Scale{x: 1 dur: 1.5 from: 1 to: 2 loop_: true}
}
```

### Tween (Property Animation)

Use `Tween{}` to animate individual shape properties (fill, stroke, d, x, y, r, etc.):

```
// Animated path morphing
Path{d: Tween{
    dur: 2.0 loop_: true
    values: ["M 10 80 Q 50 10 100 80" "M 10 80 Q 50 150 100 80"]
} fill: #f0f}

// Animated fill color
Circle{cx: 50 cy: 50 r: 30
    fill: Tween{dur: 1.5 loop_: true from: #ff0000 to: #0000ff}
}

// Animated stroke width
Rect{x: 10 y: 10 w: 80 h: 80
    fill: false stroke: #fff
    stroke_width: Tween{dur: 2.0 loop_: true from: 1 to: 5}
}
```

Tween properties:
- `from`, `to` — start and end values
- `values` — array of keyframe values (alternative to from/to)
- `dur` — duration in seconds
- `begin` — start delay in seconds
- `loop_` — `true` for indefinite, or a number for repeat count
- `calc` — "linear" (default), "discrete", "paced", "spline"
- `fill_mode` — "remove" (default) or "freeze"

### Complete Example: App Icon with Gradients, Groups, and Filters

This example from the splash demo recreates the Makepad app icon using Vector:

```
// Define gradients
let glass_bg = Gradient{x1: 0 y1: 0 x2: 1 y2: 1
    Stop{offset: 0 color: #x556677 opacity: 0.45}
    Stop{offset: 1 color: #x334455 opacity: 0.35}
}
let brain_grad = Gradient{x1: 0.5 y1: 0 x2: 0.5 y2: 1
    Stop{offset: 0 color: #x77ccff}
    Stop{offset: 0.4 color: #x7799ee}
    Stop{offset: 0.75 color: #x8866dd}
    Stop{offset: 1 color: #x9944cc}
}
let brain_glow = RadGradient{cx: 0.5 cy: 0.45 r: 0.45
    Stop{offset: 0 color: #x4466ee opacity: 0.4}
    Stop{offset: 1 color: #x4466dd opacity: 0.0}
}

// Define filter
let icon_shadow = Filter{
    DropShadow{dx: 0 dy: 4 blur: 6 color: #x000000 opacity: 0.5}
}

Vector{width: 256 height: 256 viewbox: vec4(0 0 256 256)
    // Glass background with shadow
    Rect{x: 16 y: 16 w: 224 h: 224 rx: 44 ry: 44
        fill: glass_bg filter: icon_shadow}

    // Brain glow
    Circle{cx: 128 cy: 95 r: 80 fill: brain_glow}

    // Brain paths (scaled and translated group)
    Group{transform: [Translate{x: 36.8 y: 11.4} Scale{x: 7.6 y: 7.6}]
        Path{d: "M15.5 13a3.5 3.5 0 0 0 -3.5 3.5v1a3.5 3.5 0 0 0 7 0v-1.8"
            fill: false stroke: brain_grad stroke_width: 0.35
            stroke_linecap: "round" stroke_linejoin: "round"}
        Path{d: "M8.5 13a3.5 3.5 0 0 1 3.5 3.5v1a3.5 3.5 0 0 1 -7 0v-1.8"
            fill: false stroke: brain_grad stroke_width: 0.35
            stroke_linecap: "round" stroke_linejoin: "round"}
    }

    // Keyboard keys
    Rect{x: 73 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
    Rect{x: 85 y: 190 w: 9 h: 6 rx: 1 ry: 1 fill: #xffffff fill_opacity: 0.18}
}
```

### SVG Icons in Vector

Simple SVG icons can be embedded directly as `Path` shapes:

```
// File icon (from icon_file.svg)
Vector{width: 32 height: 32 viewbox: vec4(0 0 49 49)
    Path{d: "M12.069,11.678c0,-2.23 1.813,-4.043 4.043,-4.043l10.107,0l0,8.086c0,1.118 0.903,2.021 2.021,2.021l8.086,0l0,18.193c0,2.23 -1.813,4.043 -4.043,4.043l-16.171,0c-2.23,0 -4.043,-1.813 -4.043,-4.043l0,-24.257Zm24.257,4.043l-8.086,0l0,-8.086l8.086,8.086Z"}
}

// Folder icon
Vector{width: 32 height: 32 viewbox: vec4(0 0 49 49)
    Path{d: "M11.884,37.957l24.257,0c2.23,0 4.043,-1.813 4.043,-4.043l0,-16.172c0,-2.23 -1.813,-4.042 -4.043,-4.042l-10.107,0c-0.638,0 -1.238,-0.297 -1.617,-0.809l-1.213,-1.617c-0.765,-1.017 -1.965,-1.617 -3.235,-1.617l-8.085,0c-2.23,0 -4.043,1.813 -4.043,4.043l0,20.214c0,2.23 1.813,4.043 4.043,4.043Z"}
}
```

### Vector vs Svg Widget

| | `Vector{}` | `Svg{}` |
|---|---|---|
| **Input** | Declarative shapes in Splash script | External `.svg` file via resource handle |
| **Use case** | Programmatic/inline vector graphics | Loading pre-made SVG assets |
| **Gradients** | `let` bindings, referenced by name | Parsed from SVG `<defs>` |
| **Animation** | `Tween{}` on properties, animated transforms | Parsed from SVG `<animate>` elements |
| **Custom shaders** | `shader_id` + custom `get_color` on Svg | Same mechanism via `draw_svg +:` |
| **Syntax** | `Vector{viewbox: ... Path{} Rect{}}` | `Svg{draw_svg +: {svg: crate_resource("self://file.svg")}}` |

Use `Vector{}` when you want to define graphics inline in your UI script. Use `Svg{}` when loading existing SVG files as assets.

### Hex Color Escaping Reminder

When using hex colors containing the letter `e` inside `script_mod!`, use the `#x` prefix to avoid parse errors:
```
// These need #x prefix (contain 'e' adjacent to digits)
fill: #x2ecc71
fill: #x1e1e2e
fill: #x4466ee

// These are fine without #x (no 'e' issue)
fill: #ff4444
fill: #00ff00
```

## MathView Widget (LaTeX Math Rendering)

The `MathView{}` widget renders LaTeX mathematical equations using vector glyph rendering with the NewCMMath font.

### Basic Usage

```
MathView{text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}" font_size: 14.0}
```

### Properties

| Property | Type | Default | Notes |
|----------|------|---------|-------|
| `text` | string | "" | LaTeX math expression |
| `font_size` | f64 | 11.0 | Font size in points |
| `color` | vec4 | #fff | Color of rendered math |
| `width` | Size | Fit | Widget width |
| `height` | Size | Fit | Widget height |

### Examples

```
// Inline in a layout
View{flow: Down height: Fit spacing: 12 padding: 15

    Label{text: "Quadratic Formula" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}" font_size: 14.0}

    Label{text: "Euler's Identity" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "e^{i\\pi} + 1 = 0" font_size: 16.0}

    Label{text: "Integral" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}" font_size: 14.0}

    Label{text: "Matrix" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}" font_size: 14.0}

    Label{text: "Sum" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}" font_size: 14.0}

    Label{text: "Maxwell's Equations" draw_text.color: #aaa draw_text.text_style.font_size: 10}
    MathView{text: "\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}" font_size: 14.0}
}
```

### Different Sizes

```
View{width: Fill height: Fit flow: Right spacing: 15 align: Align{y: 0.5}}
MathView{text: "\\alpha + \\beta" font_size: 8.0}
MathView{text: "\\alpha + \\beta" font_size: 12.0}
MathView{text: "\\alpha + \\beta" font_size: 18.0}
MathView{text: "\\alpha + \\beta" font_size: 24.0}
```

### Supported LaTeX

**Fractions & Roots:**
`\frac{a}{b}`, `\dfrac{a}{b}`, `\tfrac{a}{b}`, `\sqrt{x}`, `\sqrt[n]{x}`

**Subscripts & Superscripts:**
`x_i`, `x^2`, `x_i^2`, `a_{n+1}`

**Greek Letters (lowercase):**
`\alpha`, `\beta`, `\gamma`, `\delta`, `\epsilon`, `\zeta`, `\eta`, `\theta`, `\lambda`, `\mu`, `\nu`, `\xi`, `\pi`, `\rho`, `\sigma`, `\tau`, `\phi`, `\chi`, `\psi`, `\omega`

**Greek Letters (uppercase):**
`\Gamma`, `\Delta`, `\Theta`, `\Lambda`, `\Xi`, `\Pi`, `\Sigma`, `\Phi`, `\Psi`, `\Omega`

**Big Operators:**
`\sum`, `\prod`, `\int`, `\iint`, `\iiint`, `\oint`, `\bigcup`, `\bigcap`, `\bigoplus`, `\bigotimes`

**Relations:**
`=`, `\neq`, `<`, `>`, `\leq`, `\geq`, `\sim`, `\approx`, `\equiv`, `\subset`, `\supset`, `\in`, `\notin`

**Arrows:**
`\leftarrow`, `\rightarrow`, `\leftrightarrow`, `\Leftarrow`, `\Rightarrow`, `\Leftrightarrow`, `\mapsto`

**Accents:**
`\hat{x}`, `\bar{x}`, `\tilde{x}`, `\vec{x}`, `\dot{x}`, `\ddot{x}`, `\overline{x}`, `\underline{x}`

**Delimiters (auto-sizing with \left...\right):**
`\left( \right)`, `\left[ \right]`, `\left\{ \right\}`, `\left| \right|`, `\langle \rangle`, `\lfloor \rfloor`, `\lceil \rceil`

**Matrices:**
```
\begin{pmatrix} a & b \\ c & d \end{pmatrix}   % parentheses
\begin{bmatrix} a & b \\ c & d \end{bmatrix}   % brackets
\begin{vmatrix} a & b \\ c & d \end{vmatrix}   % determinant bars
\begin{cases} a & \text{if } x > 0 \\ b & \text{otherwise} \end{cases}
```

**Styles:**
`\mathbf{x}` (bold), `\mathit{x}` (italic), `\mathrm{x}` (roman), `\mathcal{x}` (calligraphic), `\mathbb{R}` (blackboard bold), `\mathfrak{g}` (fraktur)

**Spacing:**
`\,` (thin), `\:` (medium), `\;` (thick), `\!` (negative thin), `\quad`, `\qquad`

**Text & Operators:**
`\text{...}`, `\sin`, `\cos`, `\tan`, `\log`, `\ln`, `\exp`, `\lim`, `\min`, `\max`, `\det`

**Dots:**
`\ldots`, `\cdots`, `\vdots`, `\ddots`

**Misc Symbols:**
`\infty`, `\partial`, `\nabla`, `\forall`, `\exists`, `\emptyset`, `\pm`, `\mp`, `\times`, `\div`, `\cdot`

### Notes

- MathView is read-only — no selection or editing
- Uses Display math style (large operators centered)
- Backslashes must be escaped as `\\` in Splash strings
- The widget sizes itself to fit the rendered equation by default (`width: Fit`, `height: Fit`)
- An empty `text` produces no output

## MapView Widget (Geographic Map)

The `MapView{}` widget renders interactive geographic maps with OpenStreetMap data. It supports panning, zooming, street labels, and light/dark themes.

### ⛔ CRITICAL: MapView Uses Custom GPU Drawing — Batching Rules Apply ⛔

MapView renders map tiles using custom GPU geometry (`DrawMapVector`) and a full-rect background (`DrawColor`). Because of Makepad's draw batching, the map's draw calls can merge with sibling widgets' draw calls, causing the **map to render on top of other UI elements** (headers, labels, buttons) even though they appear earlier in the widget tree.

**You MUST wrap MapView in a container with `new_batch: true`** whenever MapView has sibling widgets (headers, toolbars, labels, etc.) in the same parent:

```
// CORRECT — new_batch on parent prevents map from drawing over header
RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 0
    new_batch: true
    draw_bg.color: #1a1a2e
    draw_bg.border_radius: 12.0

    RoundedView{
        width: Fill height: Fit
        padding: 12
        new_batch: true
        draw_bg.color: #16213e
        draw_bg.border_radius: 12.0
        Label{text: "Map Explorer" draw_text.color: #xeee draw_text.text_style.font_size: 13}
    }

    MapView{
        width: Fill height: 500
        dark_theme: true
    }
}
```

```
// WRONG — map draws over the header label because parent lacks new_batch
RoundedView{
    flow: Down
    RoundedView{
        Label{text: "Header" draw_text.color: #fff}
    }
    MapView{width: Fill height: 500}
}
```

### ⛔⛔⛔ CRITICAL: MapView MUST Use a Fixed Pixel Height ⛔⛔⛔

MapView has NO intrinsic content size. It cannot size itself. You MUST give it a **fixed pixel height**:

```
// CORRECT — always use a fixed pixel height
MapView{width: Fill height: 500}

// WRONG — height: Fill in a Fit parent = 0px = invisible map
MapView{width: Fill height: Fill}

// WRONG — MapView has no intrinsic height, Fit = 0px = invisible map
MapView{width: Fill height: Fit}
```

**Why not `height: Fill`?** Your output renders inside a `height: Fit` container. `Fill` inside `Fit` = circular dependency = **0 height = invisible**. This is the #1 MapView mistake.

**Why not `height: Fit`?** MapView renders tiles on a canvas — it has no content to measure, so `Fit` = 0 height = invisible.

**Always use a fixed height like `height: 400`, `height: 500`, or `height: 600`.**

### Basic Usage

```
MapView{
    width: Fill
    height: 500
}
```

This creates a map centered on Amsterdam at zoom level 14 with the default light theme.

### Properties

| Property | Type | Default | Notes |
|----------|------|---------|-------|
| `center_lon` | f64 | 4.9041 | Longitude of map center |
| `center_lat` | f64 | 52.3676 | Latitude of map center |
| `zoom` | f64 | 14.0 | Initial zoom level |
| `min_zoom` | f64 | 11.0 | Minimum zoom allowed |
| `max_zoom` | f64 | 17.0 | Maximum zoom allowed |
| `dark_theme` | bool | false | Light or dark theme |
| `use_network` | bool | false | Enable online Overpass API |
| `use_local_mbtiles` | bool | true | Enable local .mbtiles tiles |
| `width` | Size | Fill | Widget width |
| `height` | Size | — | **Must be a fixed pixel value** (e.g. `500`). Never `Fit` or `Fill`. |

**Source mode rules:** You can only use ONE source at a time. If both `use_network` and `use_local_mbtiles` are `true`, the widget silently forces offline-only mode. If both are `false`, it silently forces offline mode. To use network, set `use_network: true` AND `use_local_mbtiles: false`.

### Interactions

- **Click + Drag** — Pan the map
- **Scroll wheel** — Zoom in/out (centered on cursor)
- **T key** — Toggle light/dark theme

### Examples

```
// Default map (Amsterdam, light theme, offline tiles)
MapView{width: Fill height: 500}

// Custom center and zoom (e.g. New York — requires network)
MapView{
    width: Fill height: 500
    center_lon: -73.9857
    center_lat: 40.7484
    zoom: 15.0
    use_network: true
    use_local_mbtiles: false
}

// Dark theme
MapView{
    width: Fill height: 500
    dark_theme: true
}

// Map with header bar (note new_batch on both parent and header)
RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 0
    new_batch: true
    draw_bg.color: #1a1a2e
    draw_bg.border_radius: 12.0

    RoundedView{
        width: Fill height: Fit
        padding: Inset{top: 12 bottom: 12 left: 16 right: 16}
        flow: Right
        align: Align{y: 0.5}
        new_batch: true
        draw_bg.color: #16213e
        draw_bg.border_radius: 12.0
        Label{text: "Map Explorer" draw_text.color: #xeee draw_text.text_style.font_size: 13}
        Filler{}
        Label{text: "scroll to zoom | drag to pan | T = theme" draw_text.color: #556 draw_text.text_style.font_size: 9}
    }

    MapView{
        width: Fill height: 500
        dark_theme: true
    }
}

// Sidebar + map using Splitter
Splitter{
    axis: SplitterAxis.Horizontal
    align: SplitterAlign.FromA(300.0)
    a := SolidView{
        width: Fill height: Fill
        draw_bg.color: #222
        flow: Down padding: 15 spacing: 10
        new_batch: true
        Label{text: "Map Controls" draw_text.color: #fff draw_text.text_style.font_size: 14}
        Label{text: "Drag to pan, scroll to zoom" draw_text.color: #888 draw_text.text_style.font_size: 10}
        Label{text: "Press T to toggle theme" draw_text.color: #888 draw_text.text_style.font_size: 10}
    }
    b := MapView{width: Fill height: Fill}
}
```

### Data Sources

**Offline mode** (default): Loads tiles from a local `.mbtiles` file at `local/noord-holland-shortbread-1.0.mbtiles`. Covers the Noord Holland region (Amsterdam area). Zoom range 0–14.

**Online mode** (`use_network: true, use_local_mbtiles: false`): Queries the Overpass API for OpenStreetMap data. Covers any location worldwide but requires internet and may be slower.

**⚠️ Only one source at a time.** Setting both `use_network: true` and `use_local_mbtiles: true` silently disables network. For non-Amsterdam locations, you MUST set `use_network: true` AND `use_local_mbtiles: false`.

### Theme Styling

MapView includes built-in light and dark themes with colors for:
- **Buildings** — residential, commercial structures
- **Water** — rivers, lakes, seas
- **Land use** — residential, forest, industrial, commercial areas
- **Leisure** — parks, gardens, pitches
- **Roads** — motorways, primary, secondary, tertiary, residential, footways
- **Waterways** — rivers, canals, streams
- **Railways** — rail lines with dashed styling
- **Labels** — street names placed along road paths

### Notes

- MapView defaults to offline mode with pre-bundled Amsterdam region tiles
- Street labels appear at zoom level 13 and above
- **Always use a fixed pixel height** (e.g. `height: 500`) — both `height: Fit` and `height: Fill` produce 0px (invisible) in the splash rendering context
- **Never use `height: Fit`** — MapView has no intrinsic height; it collapses to 0
- **Never use `height: Fill`** — the splash output renders inside a `Fit` parent, so `Fill` inside `Fit` = 0 height
- **Always set `new_batch: true`** on any parent container that has MapView alongside other widgets (Labels, buttons, etc.) — otherwise the map's GPU draws will render on top of sibling UI
- For online mode, set `use_network: true` AND `use_local_mbtiles: false`
- For locations outside Amsterdam in offline mode, nothing will render — you must enable network mode
- The `zoom` value is clamped to `[min_zoom, max_zoom]` automatically
