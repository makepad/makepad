These are all the layout attributes that control how widgets affect their child elements and how they embed themselves in parent elements.
### abs_pos (Dvec2)
Absolute positioning of layout elements. Usage is not advised.
### align ([Align](Align.md))
Controls the placement of child-elements.
### clip_x (bool = true)
Boolean flag for horizontal clipping.
### clip_y (bool = true)
Boolean flag for vertical clipping.
### flow ([Flow](Flow.md))
Determines how children are laid out (i.e. vertically or horizontally)
### height ([Size](Size.md))
Determines the height of elements.
**Options**
- Absolute float(f64) dimensions
- Fill: spans the element to the full width of its parent container.
- Fit: shrinks the element to the width of its child elements.
### margin (margin)
Sets the margin outside the element.
### padding ([Padding](ft_padding.md))
Sets the padding inside the element.
### scroll (Dvec2)
The scroll position.
### spacing (f64)
The amount of spacing between stacked elements.
### width ([Size](Size.md))
Determines the width of elements.
**Options**
- Absolute float(f64) dimensions
- Fill: spans the element to the full width of its parent container.
- Fit: shrinks the element to the width of its child elements.