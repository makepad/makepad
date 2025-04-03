# Start of Selection
`TextFlow` is a versatile widget in the Makepad UI framework that facilitates rich text rendering with various formatting options, including bold, italic, underline, strikethrough, inline code, and block quotes. It supports the rendering of text blocks with customizable layouts and decorations, enabling developers to create complex and visually appealing text interfaces.

## Types

### FlowBlockType
Defines the different types of blocks that can be rendered within the `TextFlow` widget.

#### Code
Highlights text as code, typically rendered in a monospace font within a block that has a distinct background to differentiate it from regular text.

#### InlineCode
Highlights text as inline code, rendered with a monospace font and a subtle backdrop, allowing the code to seamlessly integrate within the flow of regular text.

#### Quote
Formats text as a quote, usually by indenting it and applying specific background and foreground colors to distinguish it from the surrounding content.

#### Sep
Represents a separator block, such as a horizontal line or another visual divider, to separate different sections or types of content within the text flow.

#### Strikethrough
Applies a strikethrough decoration to text, commonly used to indicate deleted or deprecated content.

#### Underline
Applies an underline decoration to text, emphasizing the content without altering its appearance significantly.

## Attributes

### DrawFlowBlock
Represents a drawable block within the `TextFlow` widget. This struct handles the rendering of individual block types (e.g., code, quote, separators) within the text flow.

#### block_type ([FlowBlockType](FlowBlockType.md))
Determines the type of block being rendered. This affects the styling and layout applied to the block.

#### code_color (Vec4)
Specifies the color used to render both code blocks and inline code, ensuring consistent theming across different code segments.

#### draw_super ([DrawQuad](DrawQuad.md))
Inherits properties from the `DrawQuad` component, enabling the rendering of quads behind the text or block content to serve as backgrounds or borders.

#### line_color (Vec4)
Defines the color used for rendering underlines and strikethroughs, allowing for customizable text decorations.

#### quote_bg_color (Vec4)
Sets the background color for quote blocks, providing visual separation from the main text and enhancing readability.

#### quote_fg_color (Vec4)
Specifies the foreground color of text within quote blocks, ensuring that quoted text stands out appropriately against its background.

#### sep_color (Vec4)
Determines the color used for rendering separators, allowing them to blend or contrast with the surrounding content based on design requirements.

### TextFlow
The primary widget struct that manages the rendering of text and text blocks with various decorations and layouts.

#### code_layout ([Layout](Layout.md))
Defines the layout properties specifically for code blocks within the `TextFlow`, controlling aspects like padding, margins, and alignment.

#### code_walk ([Walk](Walk.md))
Specifies the flow behavior of code blocks within the layout, determining how they wrap and align relative to other content.

#### draw_block ([DrawFlowBlock](ft_drawflowblock.md))
The `DrawFlowBlock` instance responsible for handling the rendering of different block types such as quotes, code blocks, and separators.

#### draw_bold ([DrawText](DrawText.md))
Handles the rendering of bold text, applying the necessary font weight and styling to emphasize content.

#### draw_bold_italic ([DrawText](DrawText.md))
Manages the rendering of text that is both bold and italic, combining the effects of both styles for enhanced emphasis.

#### draw_fixed ([DrawText](DrawText.md))
Used for rendering text in a fixed-width (monospace) font, typically utilized for code snippets or other technical content.

#### draw_italic ([DrawText](DrawText.md))
Handles the rendering of italicized text, slanting the text to emphasize or differentiate it from regular content.

#### draw_normal ([DrawText](DrawText.md))
Responsible for rendering regular, unstyled text within the `TextFlow`, serving as the default text style.

#### font_size (f64)
Specifies the default font size for rendering text within the flow, ensuring consistency across different text segments.

#### inline_code_margin (Margin)
Defines the margin surrounding inline code blocks, providing spacing to separate code from regular text for better readability.

#### inline_code_padding ([Padding](ft_padding.md))
Sets the padding around inline code blocks, ensuring that the code is visually separated from adjacent text without altering the overall layout.

#### list_item_layout ([Layout](Layout.md))
Defines the layout properties for list items within the text flow, controlling aspects like indentation, spacing, and alignment.

#### list_item_walk ([Walk](Walk.md))
Specifies how list items should flow within the layout, determining their wrapping and positioning relative to other content.

#### quote_layout ([Layout](Layout.md))
Sets the layout properties for quote blocks, controlling spacing, alignment, and indentation to properly format quoted text.

#### quote_walk ([Walk](Walk.md))
Determines the flow behavior of quote blocks within the layout, handling how they wrap and align in the context of surrounding content.

#### sep_walk ([Walk](Walk.md))
Specifies the flow behavior of separator blocks, controlling their positioning and alignment within the text flow.

```