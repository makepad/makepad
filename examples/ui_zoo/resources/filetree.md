## FileTree
A widget that displays a file tree, providing access to the file system.

### Attributes

- scroll_bars (ScrollBars)
- scroll_bars (ScrollBars)
- node_height (float)
- clip_x (bool)
- clip_y (bool)
- file_node (FileTreeNode)
- folder_node (FileTreeNode)


## FileTreeNode
### Attributes
- is_folder (bool)
- indent_width (float)
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