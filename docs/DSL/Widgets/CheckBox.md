The `CheckBox` widget provides a control for user input in the form of a checkbox. It allows users to select or deselect options.

## [Layouting](Layouting.md)
Complete layouting feature set support.

## Draw Shaders

### `draw_check` (`DrawCheckBox`)

References the `DrawShader` that determines the appearance of the `CheckBox`.

#### CheckType

The available graphical representations of the checkbox:

- **Check**: A checkmark.
- **Toggle**: A pill-shaped toggle control with an animated circle that signifies the state.
- **None**: No graphic, used for textual checkboxes.

### `draw_icon` (`DrawIcon`)

Displays a monochrome SVG vector next to the checkbox's text or instead of it.

### `draw_text` (`DrawText`)

Allows styling of the checkbox's text with all of the attributes supported by [`DrawText`](DrawText.md), including colors, font, font size, and so on.

## Fields

### bind (`String`)
Binds the checkbox state to a data model, enabling two-way data binding. This synchronizes the `CheckBox` with a variable in the application's data model, so changes in the data model update the checkbox state, and interactions with the checkbox update the data model.



### text (RcStringMut)
The text label of the checkbox.

### icon_walk (`Walk`)
Controls the icon's inner layout properties as supported by [`Walk`](Walk.md).

### label_align (`Align`)
Controls the placement of the label in its parent container according to the attributes supported by [`Align`](Align.md).

### label_walk (`Walk`)
Controls the label's inner layout properties as supported by [`Walk`](Walk.md).

## States

| State            | Trigger                               |
| :--------------- | :------------------------------------ |
| `hover` (f32)    | User moves the mouse over the element |
| `selected` (f32) | Element is selected                   |

## Widget presets & variations
- **CheckBoxToggle**: A toggle CheckBox
- **CheckBoxCustom**: Allows showing a custom icon that can be toggled.
	- draw_icon
		- svg_file

## Examples

### Basic

```rust
<CheckBox> {
    text: "Spellcheck",
}
```

### Typical

```rust
<CheckBox> {
    text: "Spellcheck",
    draw_check: { check_type: Toggle }, // Select checkbox type
}
```

### Advanced

```rust
MyCheckbox = <CheckBox> {
    draw_check: {
        // Shader object that draws the checked graphic
        instance border_width: 1.0,
        instance border_color: #x06,
        instance border_color2: #xFFFFFF0A,
        size: 8.5,
        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            let sz = self.size;
            let left = sz + 1.0;
            let c = vec2(left + sz, self.rect_size.y * 0.5);
            sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);

            sdf.stroke_keep(
                mix(self.border_color, self.border_color2, clamp(self.pos.y - 0.2, 0.0, 1.0)),
                self.border_width
            );

            sdf.fill(
                mix(
                    mix(#00000044, #00000044 * 0.1, pow(self.pos.y, 1.0)),
                    mix(#00000044 * 1.75, #00000044 * 0.1, pow(self.pos.y, 1.0)),
                    self.hover
                )
            );
            let isz = sz * 0.65;
            sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
            sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
            sdf.subtract();
            sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
            sdf.blend(self.selected);
            sdf.fill(mix(#xFFF8, #xFFFC, self.hover));
            return sdf.result;
        }
    },

    draw_icon: {
        // Shader object that draws the icon
        svg_file: dep("crate://self/resources/icons/back.svg"),
        // Icon file dependency

        fn get_color(self) -> vec4 {
            // Overwrite the shader's fill method
            return mix(
                mix(self.color, mix(self.color, #f, 0.5), self.hover),
                self.color_pressed,
                self.pressed
            );
        }
    },

    draw_text: {
        // Shader object that draws the text
        wrap: Word, // Wraps text between words
        text_style: {
            // Controls the appearance of text
            font: { path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf") },
            // Font file dependency
            font_size: 12.0, // Font size of 12.0
        },

        fn get_color(self) -> vec4 {
            // Overwrite the shader's fill method
            return mix(
                mix(self.color, self.color_hover, self.hover),
                self.color_pressed,
                self.pressed
            );
        }
    },

    bind: "my_data_model.some_boolean", // Bind to data model
    text: "Spellcheck",

    icon_walk: {
        // The icon’s inner layout properties
        width: 12.0,
        height: 12.0,
        margin: 20.0,
    },

    label_align: { x: 0.0, y: 0.5 }, // Placement of the label in its parent container

    label_walk: {
        // The label’s inner layout properties
        width: Fit,
        height: Fit,
        margin: { left: 20.0, right: 5.0 },
    },

    animator: {
        hover = {
            default: off,
            off = {
                from: { all: Forward { duration: 0.15 } },
                apply: {
                    draw_check: { hover: 0.0 },
                    draw_text: { hover: 0.0 },
                    draw_icon: { hover: 0.0 },
                }
            },
            on = {
                from: { all: Snap },
                apply: {
                    draw_check: { hover: 1.0 },
                    draw_text: { hover: 1.0 },
                    draw_icon: { hover: 1.0 },
                }
            }
        },
        focus = {
            default: off,
            off = {
                from: { all: Snap },
                apply: {
                    draw_check: { focus: 0.0 },
                    draw_text: { focus: 0.0 },
                    draw_icon: { focus: 0.0 },
                }
            },
            on = {
                from: { all: Snap },
                apply: {
                    draw_check: { focus: 1.0 },
                    draw_text: { focus: 1.0 },
                    draw_icon: { focus: 1.0 },
                }
            }
        },
        selected = {
            default: off,
            off = {
                from: { all: Forward { duration: 0.1 } },
                apply: {
                    draw_check: { selected: 0.0 },
                    draw_text: { selected: 0.0 },
                    draw_icon: { selected: 0.0 },
                }
            },
            on = {
                from: { all: Forward { duration: 0.0 } },
                apply: {
                    draw_check: { selected: 1.0 },
                    draw_text: { selected: 1.0 },
                    draw_icon: { selected: 1.0 },
                }
            }
        }
    },

    // Layout properties
    height: Fit,
    width: Fit,
    margin: 5.0,
    padding: 3.0,
    flow: Right,
    spacing: 5.0,
    align: { x: 0.0, y: 0.0 },
    line_spacing: 1.5,
}

<MyCheckbox> {
    text: "Active",
}