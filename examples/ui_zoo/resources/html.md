## Html
A widget that renders HTML content.

### Attributes
- body (ArcStringMut)
- ul_markers (Vec<String>)
- ol_markers (Vec<OrderedListType>)
- ol_separator (String)
- font_size: (float)
- font_color: (Color)
- inline_code_padding (Padding)
- inline_code_margin (Margin)
- a (HtmlLink)

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

#### code_layout
- align (Align)
- clip_x (bool)
- clip_y (bool)
- flow (Flow)
- padding (Padding)
- scroll (DVec2)
- spacing (float)

#### code_walk
- width (Size)
- height (Size)
- margin (Margin)

#### quote_layout
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

#### list_item_layout
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

## HtmlLink
- color: #x0000EE,
- hover_color: #x00EE00,
- pressed_color: #xEE0000,