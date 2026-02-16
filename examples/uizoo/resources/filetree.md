## FileTree
A widget that displays a file tree, providing access to the file system.

### Attributes
- clip_x (bool)
- clip_y (bool)
- draw_scroll_shadow (DrawScrollShadow)
- file_node (FileTreeNode)
- filler (DrawBgQuad)
- folder_node (FileTreeNode)
- node_height (float)
- scroll_bars (ScrollBars)

## FileTreeNode
### Attributes
- check_box (CheckBox)
- indent_width (float)
- indent_shift (float)
- is_folder (bool)
- min_drag_distance (float)

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