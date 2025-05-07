## PortalList
The PortalList widget efficiently handles large lists of items by only rendering the items currently visible in the viewport. It supports features like scrolling, flick scrolling, and alignment of items within the list. This is especially useful for implementing lists with a large number of items without compromising performance.

### Attributes
- flick_scroll_minimum (f64)
- flick_scroll_maximum (f64)
- flick_scroll_scaling (f64)
- flick_scroll_decay (f64)
- max_pull_down (f64)
- align_top_when_empty (bool)
- grab_key_focus (bool)
- drag_scrolling (bool)
- scroll_bar (ScrollBar)
- capture_overload (bool)
- keep_invisible (bool)
- auto_tail (bool)
- draw_caching (bool)
- reuse_items (bool)