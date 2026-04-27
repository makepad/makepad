# AI Chat UI Polish Notes

Date: 2026-04-24

## Current Direction

Target style: dark emerald liquid-glass desktop chat UI.

The intended structure is:

- Borderless rounded transparent window with cyan glass edge.
- Fixed left sidebar for navigation.
- Main chat workspace with one top toolbar.
- Readable output cards for prose, code, and diagrams.
- Bottom composer as one integrated glass input panel.
- Cyan accents for structure; warm gold only for primary action / slider knob.

## Current Problems Observed

### 1. Composer Input Is Not A Real Long-Text Composer

The composer visually looks like a multi-line prompt area, but implementation still uses a fixed-height `TextInput`.

Problems:

- Long pasted prompts are uncomfortable.
- There is no explicit multi-line / scroll behavior.
- Enter vs Shift+Enter semantics are not defined.

Preferred fix:

- Replace or wrap the input with a multi-line, vertically scrollable prompt editor.
- Default height: about 2 lines.
- Maximum height: about 6 lines.
- After max height, scroll internally.
- Preserve Enter-to-send if possible; use Shift+Enter for newline if supported.

### 2. Toolbar Controls Still Look Like Raw Widgets

`Backend`, `Moonshot`, `Glass`, slider, and percentage label are aligned better than before, but they still read as separate low-level controls.

Problems:

- Backend selector is too plain and visually heavy.
- Glass slider still resembles a rectangular progress bar rather than a refined control.
- Toolbar lacks a unified right-side control-group surface.

Preferred fix:

- Wrap backend selector and glass control inside one right-aligned toolbar cluster.
- Use consistent capsule heights and padding.
- Implement or style a custom thin slider: cyan rail, warm-gold filled segment, circular warm-gold knob.
- Keep percentage label fixed width to avoid layout jitter.

### 3. Sidebar Icons Are Unstable

Some Unicode symbols rendered as tofu / square glyphs in the current Label/Button font path.

Problems:

- Visual quality collapses when a glyph is missing.
- Current fallback to ASCII is safer but less polished.

Preferred fix:

- Use either plain stable ASCII labels, or
- Add small Makepad `Vector` icons for nav rows, independent of font glyph availability.

Recommended target:

- `New chat`: plus/pencil vector.
- `Search`: magnifier vector.
- `Plugins`: small command/grid vector.
- `Automation`: small clock/play vector.
- `Projects`: grid vector.
- `Settings`: gear-like simple vector or ASCII fallback.

### 4. Active Sidebar Item Needs Better Structure

The active item currently uses a single button text string.

Problems:

- Right-side decoration cannot be aligned cleanly.
- Padding and icon text are coupled.
- Text-only composition makes it hard to match the reference.

Preferred fix:

- Build active nav row as a small `View`/`RoundedView` or custom template with:
  - left icon slot
  - label slot
  - flexible spacer
  - right accent marker
- Keep the hit target as one clickable row if possible.

### 5. Output Cards Need More Layering

Code output is readable, but it is still a large dark block that dominates the UI.

Problems:

- Code card lacks header/tools polish.
- Diagram/prose/code visual hierarchy is not fully consistent.
- Large code blocks create a heavy vertical slab.

Preferred fix:

- Add a card surface around code/diagram blocks with subtle cyan border.
- Add a compact top-right utility cluster (`copy`, `x`, maybe settings icon).
- Ensure wide content scrolls inside the card.
- Keep paper diagram canvas readable; do not over-tint it.

### 6. Transparency Still Needs Layer Discipline

The UI is closer to the target, but transparency is still mostly uniform.

Problems:

- Sidebar/main/composer do not yet feel like distinct glass layers.
- Background wallpaper can compete with UI text.

Preferred fix:

- Outer shell: about 0.88-0.92 alpha.
- Sidebar: slightly deeper than shell.
- Main workspace: slightly deeper than shell.
- Composer: deepest glass surface with cyan edge.
- Keep edge transparency/glow, not full-panel low contrast.

### 7. Window Edge Can Be Refined Later

Current rounded edge is acceptable and should not be overworked immediately.

Avoid:

- Multiple nested neon outlines.
- Large decorative inner rectangles.
- Over-bright flowing borders that distract from content.

Preferred:

- One stable cyan border.
- Optional subtle corner highlight.
- Optional tiny resize grip if it remains visually quiet.

## Recommended Priority

1. Implement real long-text composer.
2. Replace sidebar glyphs with stable vector/ASCII icons.
3. Build toolbar control group and refine slider.
4. Add structured active nav row.
5. Refine output card surfaces and utility controls.
6. Final transparency/border tuning.

