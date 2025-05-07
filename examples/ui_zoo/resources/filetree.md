## FileTree
A widget that displays a file tree, providing access to the file system.

### Attributes
- clip_x (bool)
- clip_y (bool)
- file_node (Option<LivePtr>)
- filler (DrawBgQuad)
- folder_node (Option<LivePtr>)
- node_height (f64)
- scroll_bars (ScrollBars)
- draw_scroll_shadow (DrawScrollShadow)

## FileTreeNode
### Attributes
- check_box (CheckBox)
- indent_width (f64)
- indent_shift (f64)
- is_folder (bool)
- min_drag_distance (f64)

### Styling Attributes
#### draw_bg
- color_1 (Color)
- color_2 (Color)
- color_active (Color)

#### draw_text
- color (Color)
- color_active (Color)
- text_style
    - font_size (float)
    - line_spacing (float)
    - font_family (FontFamilyId)

#### draw_icon
- color (Color)
- color_active (Color)

#### icon_walk
- width (Size)
- height (Size)
- margin (Margin)