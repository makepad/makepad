The `ScrollBars` widget provides horizontal and vertical scroll bars that can be integrated into your UI components to enable content scrolling. It allows you to control the visibility and behavior of the scroll bars individually.

## Layouting
No layouting support.

## Fields
### show_scroll_x (bool)
Determines whether the horizontal scroll bar is visible.

### show_scroll_y (bool)
Determines whether the vertical scroll bar is visible.

### scroll_bar_x ([ScrollBar](ScrollBar.md))
An instance of the `ScrollBar` widget representing the horizontal scroll bar.

### scroll_bar_y ([ScrollBar](ScrollBar.md))
An instance of the `ScrollBar` widget representing the vertical scroll bar.

## Examples
### Advanced 
```Rust
MyScrollBars = <ScrollBars> {
  show_scroll_x: true,
  show_scroll_y: true,
  scroll_bar_x: <ScrollBar> {},
  scroll_bar_y: <ScrollBar> {}
}

<MyScrollBars> {}
```
