## PortalList
The PortalList widget efficiently handles large lists of items by only rendering the items currently visible in the viewport. It supports features like scrolling, flick scrolling, and alignment of items within the list. This is especially useful for implementing lists with a large number of items without compromising performance.

### Attributes
- align_top_when_empty (bool)
- auto_tail (bool)
- bounce_at_end (bool)
- bounce_at_start (bool)
- capture_overload (bool)
- drag_scrolling (bool)
- draw_caching (bool)
- emit_scroll_actions (bool)
- flick_scroll_minimum (float)
- flick_scroll_maximum (float)
- fling_decel (float)
- grab_key_focus (bool)
- keep_invisible (bool)
- reached_end_margin (int)
- reached_start_margin (int)
- reuse_items (bool)
- scroll_bar (ScrollBar)

### Overscroll
`bounce_at_start` and `bounce_at_end` control whether the content rubber-bands past
each edge; the stretch follows the gesture's momentum rather than a fixed cap. They
replace the old `max_pull_down` distance cap — `max_pull_down: 0.0` becomes
`bounce_at_start: false`. Fling deceleration is tuned with `fling_decel`.