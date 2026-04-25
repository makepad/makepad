spec: task
name: "AI Chat Liquid Glass UI"
tags: [makepad, aichat, ui, liquid-glass]
---

## Intent

Refine `makepad-example-aichat` into a dark emerald liquid-glass desktop chat UI
matching the approved reference direction: a semi-transparent borderless window,
left navigation rail, main chat workspace, unified top toolbar, scrollable output
cards, and an integrated composer. The UI must remain usable over visible desktop
backgrounds by balancing transparency with high-contrast text, structured panels,
and restrained cyan/gold accents.

## Decisions

- The visual language is "deep emerald glass": dark green translucent panels, cream text, cyan glow borders, and limited warm-gold emphasis.
- The outer window remains borderless and draggable, with rounded cyan glass edging and no native red/yellow/green traffic-light buttons.
- Default glass opacity is between 0.85 and 0.92; users can adjust it through a visible `Glass` slider in the top toolbar.
- The top toolbar is one horizontal structure: left title, right `Backend` selector, `Glass` slider, and percentage label.
- The sidebar is a fixed-width navigation rail with logo/title, workspace subtitle, nav items, conversation metadata, and settings at the bottom.
- The active sidebar item uses a cyan-tinted glass capsule, subtle glow, and a small decorative sparkle marker.
- Main outputs render as cards with enough opacity for readability; code blocks and diagram blocks must support horizontal scrolling.
- The composer is a single integrated panel at the bottom of the main workspace, not detached buttons floating over the window.
- `Clear` and send are part of the composer action group; send uses the warm-gold accent, while secondary actions stay low-contrast.
- Implementation uses Makepad-native widgets, layout, shaders, and theme constants; no webview, CSS, SVG conversion path, or new UI framework.

## Boundaries

### Allowed Changes

- examples/aichat/src/main.rs
- examples/aichat/resources/**
- examples/aichat/specs/**
- examples/aichat/README.md
- widgets/src/markdown.rs

### Forbidden

- Do not reintroduce native macOS traffic-light controls.
- Do not add a webview, HTML/CSS renderer, or external GUI framework.
- Do not remove existing chat backend selection behavior.
- Do not remove diagram rendering or code-fence rendering support.
- Do not make the whole window so transparent that text contrast depends on wallpaper brightness.
- Do not implement new AI backend protocol behavior as part of this UI task.

### Out of Scope

- New chat history persistence model.
- New model-provider APIs.
- Accessibility overhaul beyond contrast and visible focus states.
- Full design-system extraction into a reusable crate.
- Animation-heavy particle effects or decorative effects that risk performance.

## Acceptance Criteria

Scenario: Borderless liquid-glass shell
  Test:
    Package: makepad-example-aichat
    Filter: aichat_liquid_glass_shell_contract
  Given the AI Chat window is launched
  When the root window shell is rendered
  Then no native traffic-light buttons are visible
  And the shell has a rounded outer shape
  And the shell has a cyan/blue glass border or glow
  And the shell background opacity is between 0.85 and 0.92 by default
  And the top drag region still allows moving the window

Scenario: Unified top toolbar
  Test:
    Package: makepad-example-aichat
    Filter: aichat_top_toolbar_contract
  Given the main workspace is rendered
  When the top toolbar is inspected
  Then the left side contains the `AI Chat` title
  And the right side contains `Backend`, the current backend selector, `Glass`, the opacity slider, and a percentage label
  And all toolbar controls share a consistent height, vertical alignment, padding, and text color
  And the backend selector and glass slider are not rendered as detached debug controls

Scenario: Glass opacity can be adjusted at runtime
  Test:
    Package: makepad-example-aichat
    Filter: aichat_glass_opacity_slider_contract
  Given the window is visible with default opacity
  When the user drags the `Glass` slider
  Then the percentage label updates to the selected value
  And the shell, sidebar, main area, and composer opacity update together
  And text remains readable at the minimum supported opacity
  And the app does not crash or lose chat state

Scenario: Sidebar matches the target navigation rail
  Test:
    Package: makepad-example-aichat
    Filter: aichat_sidebar_contract
  Given the sidebar is rendered
  When the nav rail is inspected
  Then it has a fixed width and full-height layout
  And it contains the app mark, `AI Chat` title, and `Diagram workspace` subtitle
  And it contains nav items for new chat, search, plugins, automation, and projects
  And the active nav item is a cyan-tinted capsule with glow and a small accent marker
  And settings remains anchored at the bottom

Scenario: Main content cards preserve readability over wallpaper
  Test:
    Package: makepad-example-aichat
    Filter: aichat_output_card_readability_contract
  Given a response contains prose, code, and a diagram block
  When the content is rendered over a bright or detailed desktop wallpaper
  Then prose cards use a dark emerald translucent surface with visible borders
  And code cards use a deeper ink surface with syntax colors readable against the background
  And diagram cards keep their paper canvas readable
  And no primary text uses low-alpha grey that becomes unreadable over the wallpaper

Scenario: Wide rendered content is horizontally scrollable
  Test:
    Package: makepad-example-aichat
    Filter: aichat_horizontal_scroll_contract
  Given a diagram or code block is wider than the visible main column
  When the block is rendered
  Then it does not force the whole window wider
  And the block can scroll horizontally inside its card
  And the copy/utility controls remain reachable

Scenario: Composer is a single integrated input panel
  Test:
    Package: makepad-example-aichat
    Filter: aichat_composer_contract
  Given the user is ready to type
  When the bottom composer is rendered
  Then the input placeholder, quick action buttons, `Clear`, and send button are contained in one rounded glass panel
  And `Clear` and send are aligned as a right-side action group with consistent spacing
  And the send button uses the warm-gold accent
  And secondary quick actions use low-contrast cyan/cream styling
  And the composer width is proportional to the main workspace instead of an oversized floating bubble

Scenario: Minimum usable window size
  Test:
    Package: makepad-example-aichat
    Filter: aichat_min_window_layout_contract
  Given the user resizes the window near its minimum supported size
  When the layout recomputes
  Then the sidebar remains visible without overlapping the main workspace
  And the top toolbar does not overlap the content card
  And the composer remains visible and usable
  And overflow content is handled by scroll views rather than clipped critical controls

Scenario: UI task does not change backend behavior
  Test:
    Package: makepad-example-aichat
    Filter: aichat_backend_behavior_unchanged_contract
  Given the user selects an existing backend and sends a prompt
  When the request is submitted
  Then the same backend routing behavior is used as before this UI task
  And existing model selection state is preserved
  And diagram/code markdown rendering remains active

Scenario: Network failure remains visible and non-destructive
  Test:
    Package: makepad-example-aichat
    Filter: aichat_network_failure_ui_contract
  Level: integration
  Test Double: fake backend transport returning a connection-lost error
  Given the selected backend request fails with a network connection error
  When the app renders the failure state
  Then the liquid-glass layout remains intact
  And the error is shown as readable text inside the chat workspace
  And the composer remains enabled for retry or editing
  And no diagram, code card, toolbar, or sidebar control disappears
