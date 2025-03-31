A popup menu component that displays a list of selectable items.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## PopupMenu

### DrawShaders

#### `draw_bg` ([DrawQuad](DrawQuad.md))
Determines the appearance of the PopupMenu's background.

### Fields

#### `menu_item` (`Option<LivePtr>`)
Reference to the menu item component.

#### `items` (`Vec<String>`)
Names or labels of the menu items represented in the popup menu.

## PopupMenuItem

An individual item within the popup menu.

### DrawShaders

#### `draw_bg` ([DrawQuad](DrawQuad.md))
Defines the appearance of the background of the menu item.

#### `draw_name` ([DrawText](DrawText.md))
Allows styling of the item's text with attributes supported by [DrawText](DrawText.md), including colors, font, and font size.

### Fields

#### `indent_width` (`f32`)
The amount by which the menu item is indented relative to its parent.

#### `icon_walk` ([Walk](Walk.md))
Controls the icon’s layout properties as supported by [Walk](Walk.md).
## States

| State  | Trigger                               |
| :----- | :------------------------------------ |
| opened  | Indicates whether the menu item is opened. |
| hover  | User moves the mouse over the element |
| selected | Element is selected |

## Example

### Typical

```rust
<PopupMenuItem> {
    indent_width: 10.0,
    // Indents the menu item by 10 units relative to its parent.

    // LAYOUT PROPERTIES

    height: 25.0,
    // Element height set to 25 units.

    width: Fit,
    // Width adjusts to fit its content.
}

MyPopupMenu = <PopupMenu> {
    menu_item: <PopupMenuItem> {},
    items: ["Option 1", "Option 2", "Option 3"],
    // List of items to display in the popup menu.

    // LAYOUT PROPERTIES

    height: Fit,
    // Height adjusts to fit its content.

    width: Fit,
    // Width adjusts to fit its content.
}
```

### Advanced

```rust
MyPopupMenuItem = <PopupMenuItem> {
    indent_width: 10.0,
    // Indents the menu item by 10 units.

    draw_bg: {
        instance selected: 0.0,
        instance hover: 0.0,

        uniform color: #888888,
        uniform color_selected: #666666,
        uniform check_color: #AA0000,
        uniform check_color_hover: #FF0000,

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);

            sdf.clear(mix(
                self.color,
                self.color_selected,
                self.hover
            ));

            // Draw a checkmark when selected.
            if self.selected > 0.0 {
                let sz = 3.0;
                let dx = 2.0;
                let c = vec2(8.0, 0.5 * self.rect_size.y);
                sdf.move_to(c.x - sz + dx * 0.5, c.y - sz + dx);
                sdf.line_to(c.x, c.y + sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.stroke(mix(self.check_color, self.check_color_hover, self.hover), 1.0);
            }

            return sdf.result;
        }
    },

    icon_walk: {
        margin: 10.0,
        width: 16.0,
        height: Fit,
    },
    // Defines the layout for the icon.

    draw_name: {
        instance selected: 0.0,
        instance hover: 0.0,

        uniform color: #AAAAAA,
        uniform color_hover: #BBBBBB,
        uniform color_selected: #999999,

        text_style: {
            font: { path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf") },
            // Font file dependency.

            font_size: 12.0,
            // Font size set to 12.0.
        },

        fn get_color(self) -> vec4 {
            return mix(
                mix(
                    self.color,
                    self.color_selected,
                    self.selected
                ),
                self.color_hover,
                self.hover
            );
        }
    },

    animator: {
        hover = {
            default: off,
            off = {
                from: { all: Snap },
                apply: {
                    draw_bg: { hover: 0.0 },
                    draw_name: { hover: 0.0 },
                }
            },
            on = {
                cursor: Hand,
                from: { all: Snap },
                apply: {
                    draw_bg: { hover: 1.0 },
                    draw_name: { hover: 1.0 },
                }
            }
        },

        select = {
            default: off,
            off = {
                from: { all: Snap },
                apply: {
                    draw_bg: { selected: 0.0 },
                    draw_name: { selected: 0.0 },
                }
            },
            on = {
                from: { all: Snap },
                apply: {
                    draw_bg: { selected: 1.0 },
                    draw_name: { selected: 1.0 },
                }
            }
        }
    },

    // LAYOUT PROPERTIES

    height: 25.0,
    // Element height set to 25 units.

    width: Fit,
    // Width adjusts to fit its content.

    margin: { top: 10.0, right: 5.0, bottom: 10.0, left: 5.0 },
    // Individual margins for all four directions.

    padding: { top: 15.0, left: 0.0 },
    // Padding for top and left.

    flow: Right,
    // Stacks children from left to right.

    spacing: 10.0,
    // Spacing of 10.0 units between children.

    align: { x: 0.0, y: 1.0 },
    // Positions children at the left-bottom corner.

    clip_x: true,
    // Hides horizontal overflow.

    clip_y: false,
    // Allows vertical overflow.

    line_spacing: 1.5,

    scroll: vec2(0.0, 300.0)
}

MyPopupMenu = <PopupMenu> {
    menu_item: <MyPopupMenuItem> {},
    items: ["Option A", "Option B", "Option C"],
    // Custom menu items.

    draw_bg: {
        uniform color: #888888,
        uniform color_bevel_light: #AAAAAA,
        instance color_bevel_shadow: #444444,
        instance bevel: 2.0,

        uniform border_width: 1.0,
        uniform inset: vec4(0.0, 0.0, 0.0, 0.0),
        uniform radius: 2.0,
        uniform blur: 0.0,

        fn get_color(self) -> vec4 {
            return self.color;
        },

        fn get_border_color(self) -> vec4 {
            return mix(self.color_bevel_light, self.color_bevel_shadow, pow(self.pos.y, 0.35));
        },

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.blur = self.blur;
            sdf.box(
                self.inset.x + self.border_width,
                self.inset.y + self.border_width,
                self.rect_size.x - (self.inset.x + self.inset.z + self.border_width * 2.0),
                self.rect_size.y - (self.inset.y + self.inset.w + self.border_width * 2.0),
                max(1.0, self.radius)
            );
            sdf.fill_keep(self.get_color());
            if self.border_width > 0.0 {
                sdf.stroke(self.get_border_color(), self.bevel);
            }
            return sdf.result;
        }
    },

    // LAYOUT PROPERTIES

    height: Fit,
    width: Fit,
    margin: 0.0,
    padding: 5.0,
    flow: Down,
    spacing: 2.0,
    align: { x: 0.0, y: 0.0 },
    clip_x: true,
    clip_y: false,
    line_spacing: 1.5
}

<MyPopupMenu> {}
```
