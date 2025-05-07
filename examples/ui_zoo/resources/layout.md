These are all the layout attributes that control how widgets affect their child elements and how they embed themselves in parent elements.
### abs_pos (Vec2)
Absolute positioning of layout elements. Usage is not advised.
### align (Align)
Controls the placement of child-elements.
### clip_x (bool = true)
Boolean flag for horizontal clipping.
### clip_y (bool = true)
Boolean flag for vertical clipping.
### flow (Flow)
Determines how children are laid out (i.e. vertically or horizontally)
### height (Size)
Determines the height of elements.
**Options**
- Absolute float(f64) dimensions
- Fill: spans the element to the full width of its parent container.
- Fit: shrinks the element to the width of its child elements.
### margin (margin)
Sets the margin outside the element.
### padding (Padding)
Sets the padding inside the element.
### scroll (Vec2)
The scroll position.
### spacing (float)
The amount of spacing between stacked elements.
### width (Size)
Determines the width of elements.
**Options**
- Absolute float(f64) dimensions
- Fill: spans the element to the full width of its parent container.
- Fit: shrinks the element to the width of its child elements.