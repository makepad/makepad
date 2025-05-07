## Markdown
A widget that renders Markdown-formatted text.

### Attributes
- body (ArcStringMut)
- paragraph_spacing (f64)
- pre_code_spacing (f64)
- use_code_block_widget (bool)

### Styling Attributes
- font_size (float),
- font_color (Color),
- paragraph_spacing (float)
- pre_code_spacing (float)
- inline_code_padding (Size)
- inline_code_margin (Size)

#### draw_normal
- text_style (TextStyle)
- color (Color)

#### draw_italic
- text_style (TextStyle)
- color (Color)

#### draw_bold
- text_style (TextStyle)
- color (Color)

#### draw_bold_italic
- text_style (TextStyle)
- color (Color)

#### draw_fixed
- text_style (TextStyle)
- color (Color)

#### draw_layout
- align (Align)
- clip_x (bool)
- clip_y (bool)
- flow (Flow)
- padding (Padding)
- scroll (DVec2)
- spacing (float)

#### quote_walk
- width (Size)
- height (Size)
- margin (Margin)

### list_item_layout
- align (Align)
- clip_x (bool)
- clip_y (bool)
- flow (Flow)
- padding (Padding)
- scroll (DVec2)
- spacing (float)

#### list_item_walk
- width (Size)
- height (Size)
- margin (Margin)

#### sep_walk
- width (Size)
- height (Size)
- margin (Margin)

#### draw_block
- line_color (Color)
- sep_color (Color)
- quote_bg_color (Color)
- quote_fg_color (Color)
- code_color (Color)