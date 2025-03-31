These are all the layout attributes that control how widgets affect their child elements and how they embed themselves in parent elements.
## abs_pos (Dvec2)
Absolute positioning of layout elements. Usage is not advised.
## align ([Align](Align.md))
Controls the placement of child-elements.
## clip_x (bool = true)
Boolean flag for horizontal clipping.
## clip_y (bool = true)
Boolean flag for  clipping.
## flow ([Flow](Flow.md))
Determines how child elements are laid out (i.e. vertically or horizontally)
## height ([Size](Size.md))
Determines the height of elements.
**Options**
- float
- Fill
- Fit
## line_spacing (f64)
Spacing between text lines.
## margin (margin)
Sets the margin area on all four sides of an element.
## padding ([Padding](ft_padding.md))
Padding around the content.
## scroll (Dvec2)
The scroll position.
## spacing (f64)
Spacing between elements.
## width ([Size](Size.md))
Determines the width of elements.
**Options**
- Absolute float values
- Fill: spans the element to the full width of its parent container.
- Fit: shrinks the element to the width of its child elements.
# Best practices
## Filler elements
…