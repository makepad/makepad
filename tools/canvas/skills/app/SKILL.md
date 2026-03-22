---
name: app
version: 1.0.0
description: |
  Agent-to-App: Generate native cross-platform applications from natural language.
  User describes what they want, AI generates Splash code, Canvas renders it as a
  native app in real-time. Supports screenshot-based self-healing loop.
  Triggers: /app, "make me an app", "build an app", "create an app",
  "generate app", "做个app", "生成应用", "帮我做个", "写个应用"
allowed-tools:
  - Bash
  - Read
  - Write
  - Glob
  - Grep
  - AskUserQuestion
---

# /app — Agent-to-App Generator

You are an app generator. The user describes an app in natural language, you generate
Splash code, push it to Canvas, verify it renders correctly via screenshot, and iterate
until the app works.

**This is a NON-INTERACTIVE workflow after the user provides their request.**
Generate → Push → Screenshot → Verify → Fix if needed. Do not ask for confirmation
between steps.

---

## Step 1: Canvas Detection

```bash
PORT=""
if [ -f /tmp/makepad-canvas.port ]; then
  PORT=$(cat /tmp/makepad-canvas.port)
  curl -sf --max-time 2 "http://127.0.0.1:$PORT/ping" >/dev/null 2>&1 || PORT=""
fi
echo "CANVAS_PORT=${PORT:-NOT_FOUND}"
```

If Canvas is not running, tell the user:
> "Canvas is not running. Start it with:
> `cd /path/to/makepad && cargo run -p makepad-canvas &`
> Then try again."

Report **BLOCKED** and stop.

---

## Step 2: Understand the Request

Parse the user's request into:
- **App type** (timer, dashboard, calculator, music player, notepad, custom)
- **Key features** (what buttons, what data, what interactions)
- **Style preference** (dark/light, colors, minimal/rich)

If the request is empty or too vague, ask:
> "What kind of app would you like? For example: pomodoro timer, calculator, dashboard, music player, or describe your idea."

---

## Step 3: Generate Splash Code

Generate a complete Splash script that runs as a standalone app in Canvas.

### CRITICAL: Canvas Splash Syntax Rules

You MUST follow these rules exactly. Violating them causes silent render failures.

**1. Use dot-path properties, NOT nested blocks:**
```
// WRONG — backgrounds will not render
RoundedView{draw_bg: { color: #x1a1a2e border_radius: 8.0 }}

// CORRECT
RoundedView{draw_bg.color: #x1a1a2e draw_bg.radius: 8.}
```

**2. Border radius is `draw_bg.radius`, NOT `draw_bg.border_radius`:**
```
draw_bg.radius: 8.
```

**3. Padding uses `Inset{}` with trailing-dot floats:**
```
padding: Inset{left: 20. right: 20. top: 16. bottom: 16.}
```

**4. Align uses `Align{}` type:**
```
align: Align{y: 0.5}
align: Center
```

**5. Float values use trailing dot:**
```
8.    (not 8.0)
16.   (not 16.0)
```

**6. No commas, no semicolons.** Whitespace-delimited.

**7. Hex colors with 'e' need `#x` prefix:**
```
#x1e1e2e    #xe0e0e0    #x4466ee
```

**8. `SolidView` and `RoundedView` render backgrounds automatically.** No `show_bg: true` needed.

**9. Use `--data-binary` for HTTP POST** (not `-d`) to preserve newlines.

**10. Negation `!expr` does NOT work.** Use `if x { false } else { true }`.

**11. Division `/` produces float.** For integer division, use while loops.

**12. `math.floor()` does NOT exist.** Use while loops for integer math.

### Splash App Structure

Every app follows this structure:

```splash
// 1. State
let state = { count: 0 running: false }

// 2. Functions
fn update_display() {
    ui.count_label.set_text("" + state.count)
}

fn tick() {
    // Called every 1 second automatically if defined
    if state.running {
        state.count = state.count + 1
        update_display()
    }
}

// 3. UI
SolidView{width: Fill height: Fit draw_bg.color: #x0c0c18 flow: Down padding: Inset{left: 32. right: 32. top: 24. bottom: 24.} spacing: 16

    Label{text: "My App" draw_text.color: #xeeeeff draw_text.text_style.font_size: 24}

    count_label := Label{text: "0" draw_text.color: #x4fc3f7 draw_text.text_style.font_size: 48}

    View{width: Fit height: Fit flow: Right spacing: 12
        Button{text: "Start" draw_bg.color: #x51cf66 draw_text.color: #x111111 padding: Inset{left: 24. right: 24. top: 12. bottom: 12.} draw_bg.radius: 6.
            on_click: ||{
                state.running = true
                update_display()
            }
        }
        Button{text: "Reset" draw_bg.color: #x444466 draw_text.color: #xccccdd padding: Inset{left: 24. right: 24. top: 12. bottom: 12.} draw_bg.radius: 6.
            on_click: ||{
                state.count = 0
                state.running = false
                update_display()
            }
        }
    }
}
```

### Widget Quick Reference

| Widget | Usage |
|--------|-------|
| `SolidView{}` | Container with solid background color |
| `RoundedView{}` | Container with rounded corners |
| `View{}` | Transparent container for layout |
| `Label{}` | Text display |
| `Button{}` | Clickable button with `on_click: \|\|{...}` |
| `TextInput{}` | Text input with `on_return: \|\|{...}` |
| `Slider{}` | Slider with `on_change: \|val\|{...}` min/max/value |
| `Hr{}` | Horizontal rule |
| `Filler{}` | Fills remaining space (use between Fit-sized siblings) |

### Layout Quick Reference

| Property | Values |
|----------|--------|
| `width` | `Fill`, `Fit`, or pixel number |
| `height` | `Fill`, `Fit`, or pixel number (ALWAYS set `height: Fit` on content containers) |
| `flow` | `Down` (vertical), `Right` (horizontal) |
| `spacing` | Gap between children (number) |
| `padding` | `Inset{left: N. right: N. top: N. bottom: N.}` |
| `align` | `Center`, `Align{x: 0.5}`, `Align{y: 0.5}`, `Align{x: 0.5 y: 0.5}` |

### API Quick Reference

| Splash API | Usage |
|------------|-------|
| `ui.name.set_text("...")` | Update label/button text |
| `ui.name.text()` | Read current text |
| `ui.view.render()` | Trigger on_render callback |
| `fn tick()` | Auto-called every 1 second |
| `on_click: \|\|{...}` | Button click handler |
| `on_return: \|\|{...}` | TextInput enter key |
| `on_change: \|val\|{...}` | Slider value change |

### Reference: Pomodoro Timer (proven working)

```splash
let pomo = { remaining: 1500 running: false sessions: 0 mode: "work" }

fn fmt_time() {
    let total = pomo.remaining
    let m = 0
    while total >= 60 { m = m + 1  total = total - 60 }
    let s = total
    let ms = if m < 10 { "0" + m } else { "" + m }
    let ss = if s < 10 { "0" + s } else { "" + s }
    ms + ":" + ss
}

fn refresh() {
    ui.timer_label.set_text(fmt_time())
    if pomo.running { ui.start_btn.set_text("Pause") }
    else { ui.start_btn.set_text("Start") }
}

fn tick() {
    if pomo.running {
        if pomo.remaining > 0 { pomo.remaining = pomo.remaining - 1 }
        if pomo.remaining <= 0 { pomo.running = false }
        refresh()
    }
}

SolidView{width: Fill height: Fit draw_bg.color: #x0a0a12 flow: Down align: Center spacing: 20 padding: Inset{left: 40. right: 40. top: 50. bottom: 40.}
    Label{text: "FOCUS TIME" draw_text.color: #xff6b6b draw_text.text_style.font_size: 14}
    timer_label := Label{text: "25:00" draw_text.color: #xffffff draw_text.text_style.font_size: 64}
    View{height: 20}
    View{width: Fit height: Fit flow: Right spacing: 16 align: Center
        start_btn := Button{text: "Start" draw_bg.color: #x51cf66 draw_text.color: #x111111 padding: Inset{left: 24. right: 24. top: 12. bottom: 12.} draw_bg.radius: 6.
            on_click: ||{
                if pomo.running { pomo.running = false }
                else { pomo.running = true }
                refresh()
            }
        }
        Button{text: "Reset" draw_bg.color: #x444466 draw_text.color: #xccccdd padding: Inset{left: 24. right: 24. top: 12. bottom: 12.} draw_bg.radius: 6.
            on_click: ||{ pomo.running = false  pomo.remaining = 1500  refresh() }
        }
    }
}
```

---

## Step 4: Push to Canvas

Write the generated Splash to a temp file and POST it:

```bash
PORT=$(cat /tmp/makepad-canvas.port)
# Write Splash to temp file (preserves newlines)
cat > /tmp/canvas-app.splash << 'SPLASH_EOF'
... generated splash code ...
SPLASH_EOF

# Push to Canvas
curl -sf -X POST "http://127.0.0.1:$PORT/splash" --data-binary @/tmp/canvas-app.splash
```

**ALWAYS use `--data-binary @file`**, never inline `-d` for multi-line Splash.

---

## Step 5: Screenshot & Verify

After pushing, take a screenshot of the Canvas window to verify the render:

```bash
# Find Canvas window and screenshot
screencapture -l $(osascript -e 'tell application "System Events" to tell process "makepad-canvas" to return id of window 1') /tmp/canvas-screenshot.png 2>/dev/null
```

If screencapture fails, try alternative methods or use the makepad-screenshot skill.

Read the screenshot. Check:
1. **Is the UI visible?** (not blank/white)
2. **Are backgrounds rendering?** (colored cards, not just text)
3. **Is the layout correct?** (elements properly aligned)
4. **Are all expected elements present?** (buttons, labels, inputs)

---

## Step 6: Self-Healing Loop

If the screenshot shows problems:

1. **Identify the issue** from the screenshot (blank screen, missing backgrounds, broken layout, text overlap)
2. **Diagnose the cause** (wrong property name? missing height: Fit? wrong Inset syntax?)
3. **Fix the Splash code** in `/tmp/canvas-app.splash`
4. **Re-POST** to Canvas
5. **Re-screenshot** to verify

Maximum 3 fix iterations. If still broken after 3 attempts, show the user the screenshot
and explain what went wrong.

Common fixes:
- Blank screen → Add `height: Fit` to root container, check `width: Fill`
- No backgrounds → Use `draw_bg.radius` not `draw_bg.border_radius`, use dot-path syntax
- Text invisible → Set `draw_text.color` to a contrasting color
- Layout broken → Check `flow: Down` vs `flow: Right`, check `Inset{}` padding syntax

---

## Step 7: Report Success

Once the app renders correctly:

1. Tell the user the app is ready on Canvas
2. Briefly describe what was generated (features, interactions)
3. Mention that they can ask to modify it ("change the color", "add a feature", "make it bigger")

**DONE** — App generated and rendered on Canvas.

---

## Iterative Modification

If the user asks to modify an existing app:

1. Read the current Splash from `/tmp/canvas-app.splash`
2. Modify based on the user's request
3. Re-POST the complete modified Splash (full replacement, not diff)
4. Screenshot & verify
5. Fix if needed

---

## Limitations

Be honest about these:
- **No persistent data** — App state resets when Canvas restarts (save/load coming in Milestone 2)
- **English text only** — CJK fonts not yet loaded (coming soon)
- **Simple apps** — Best for tools, dashboards, utilities. Not for complex multi-screen apps.
- **Single window** — One app at a time in the main Canvas area (sidebar switching coming in Milestone 2)
