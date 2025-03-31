The standard text input form element.
## [Layouting](Layouting.md)
Complete layouting feature set support.
## TextInput
### DrawShaders
#### draw_bg ([DrawColor](DrawColor.md))
Determines the appearance of the `TextInput`'s background.
#### draw_label ([DrawLabel](#DrawLabel))
Determines the appearance of the text label within the `TextInput`.
#### draw_selection ([DrawQuad](DrawQuad.md))
Determines the appearance of the text selection highlight.
#### draw_cursor ([DrawQuad](DrawQuad.md))
Determines the appearance of the text cursor.
### Fields
#### label_align ([Align](Align.md))
Controls horizontal and vertical text alignment of the text label.
#### cursor_width (f64)
Controls the width of the text cursor.
#### empty_text (String)
The placeholder text displayed when the input has no content. Typically used for inline labeling or showing example data.
#### text (String)
The current text content of the input control.
### Flags
* is_read_only (bool)
If `true`, the `TextInput` is not editable.
* is_numeric_only (bool)
If `true`, restricts the input to numeric characters only.

### DrawLabel ([DrawText](DrawText.md))
Allows styling of the `TextInput`'s text with all attributes supported by [DrawText](DrawText.md), including colors, font, font size, and more.
#### Flags
* is_empty (f32)
Flag indicating whether the input field is empty (`1.0` for empty, `0.0` for not empty). Useful for conditional styling.
### States
| State       | Trigger                               |
| :---------- | :------------------------------------ |
| focus (f32) | Element gains or loses focus          |
| hover (f32) | User moves the mouse over the element |
## Examples
### Basic
```rust
<TextInput> {}
```
An empty `TextInput` with default settings.
### Typical
```rust
<TextInput> {
    empty_text: "Your Name", // Placeholder text when input is empty
    text: "John Doe",        // Initial content of the input

    // LAYOUT PROPERTIES

    height: 15.0, // Element is 15 units high
    width: Fill,  // Element expands to use all available horizontal space
}
```
A `TextInput` with placeholder text and initial content.
### Advanced
```rust
MyTextInput = <TextInput> {
    cursor_width: 1.0,                   // Custom cursor width
    empty_text: "Enter your name",       // Custom placeholder text
    label_align: { x: 0.0, y: 0.5 },     // Left-align text vertically centered
    text: "John Doe",                    // Initial content

    is_numeric_only: false,              // Allow all characters
    is_read_only: false,                 // Make the input editable

    draw_bg: {
        fn pixel(self) -> vec4 {
            return vec4(0.95, 0.95, 0.95, 1.0); // Light grey background color
        }
    },

    draw_cursor: {
        instance focus: 0.0,
        uniform border_radius: 0.5,
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(
                0.0,
                0.0,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            sdf.fill(mix(#00000000, #000000FF, self.focus)); // Cursor color changes on focus
            return sdf.result;
        }
    },

    draw_label: {
        fn get_color(self) -> vec4 {
            return mix(#888, #000, self.focus); // Text color changes on focus
        }
    },

    draw_selection: {
        instance hover: 0.0,
        instance focus: 0.0,
        uniform border_radius: 5.0,
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.box(
                0.0,
                0.0,
                self.rect_size.x,
                self.rect_size.y,
                self.border_radius
            );
            sdf.fill(
                mix(#FFFFFF00, #CCCCCCFF, self.focus) // Selection color changes on focus
            );
            return sdf.result;
        }
    },

    animator: {
        hover = {
            default: off,
            off = {
                from: { all: Forward { duration: 0.1 } },
                apply: {
                    draw_selection: { hover: 0.0 },
                    draw_label: { hover: 0.0 },
                }
            },
            on = {
                from: { all: Snap },
                apply: {
                    draw_selection: { hover: 1.0 },
                    draw_label: { hover: 1.0 },
                }
            }
        },
        focus = {
            default: off,
            off = {
                from: { all: Forward { duration: 0.25 } },
                apply: {
                    draw_cursor: { focus: 0.0 },
                    draw_bg: { focus: 0.0 },
                    draw_selection: { focus: 0.0 },
                    draw_label: { focus: 0.0 },
                }
            },
            on = {
                from: { all: Snap },
                apply: {
                    draw_cursor: { focus: 1.0 },
                    draw_bg: { focus: 1.0 },
                    draw_selection: { focus: 1.0 },
                    draw_label: { focus: 1.0 },
                }
            }
        }
    },

    // LAYOUT PROPERTIES

    height: 20.0,
    width: 150.0,
    margin: { top: 0.0, right: 5.0, bottom: 0.0, left: 5.0 },
    padding: 2.5,
    flow: Right,
    align: { x: 0.0, y: 0.0 },
    line_spacing: 1.5,
}

<MyTextInput> {}
```
An advanced `TextInput` with custom styling, animations, and properties.