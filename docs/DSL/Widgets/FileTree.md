A widget that displays a file tree, providing access to the file system.

## [Layouting](Layouting.md)
Complete layouting feature set support.

## FileTreeNode
The individual nodes in the `FileTree`, typically representing folders or files.

### DrawShaders
#### draw_bg ([DrawBgQuad](#drawbgquad))
Determines the background appearance of the node.

#### draw_icon ([DrawIconQuad](#drawiconquad))
Draws a monochrome SVG vector icon next to the node's text or instead of it.

#### draw_name ([DrawNameText](#drawnametext))
Allows styling of the node's text with all attributes supported by [DrawText](DrawText.md), including colors, font, and font size.

### Fields
#### check_box ([CheckBox](CheckBox.md))
An optional nested `CheckBox` widget within the node.

#### indent_width (f64)
Amount of horizontal space each indentation level adds to the node.

#### indent_shift (f64)
Uniform horizontal shift applied to all nodes.

#### icon_walk ([Walk](Walk.md))
Controls the icon's inner layout properties as supported by [Walk](Walk.md).

#### min_drag_distance (f64)
Minimum distance a node needs to be moved during drag and drop before it detaches from its current position.
### Flags

* is_folder (bool)
Indicates whether the node represents a folder.

### States
| State     | Trigger                               |
|-----------|---------------------------------------|
| focussed (f32) | Node has focus                        |
| hover (f32)     | User moves the mouse over the node    |
| opened (f32)    | Node is unfolded                      |
| selected (f32) | Node is selected                      |

## FileTree
The main `FileTree` widget that contains and manages `FileTreeNode` instances.

### DrawShaders
#### draw_scroll_shadow ([DrawScrollShadow](DrawScrollShadow.md))
Shows a drop shadow at the top of the container when there is hidden overflow content at the top.

#### filler ([DrawBgQuad](#drawbgquad))
Determines the appearance of the `FileTree`'s background filler.

### Fields
#### scroll_bars ([ScrollBars](ScrollBars.md))
Reference to the desired `ScrollBars` widget.

#### file_node (LivePtr)
Reference to the `FileTreeNode` widget to be used for files.

#### folder_node (LivePtr)
Reference to the `FileTreeNode` widget to be used for folders.

#### node_height (f64)
Determines the vertical space allocated to each node.

## DrawShaders

### DrawBgQuad ([DrawQuad](DrawQuad.md))
Determines the background appearance of nodes in the `FileTree`.

#### Flags
##### is_even (f32)
Indicates even rows, useful for drawing alternate lines.

##### is_folder (f32)
Indicates whether the node is a folder.

##### scale (f32)
Scale factor for the node.

#### States
| State     | Trigger                               |
|-----------|---------------------------------------|
| focussed  | Node has focus                        |
| hover     | User moves the mouse over the element |
| opened    | Node is unfolded                      |
| selected  | Node is selected                      |

### DrawNameText ([DrawText](DrawText.md))
Allows styling of the node's text.

#### Flags
##### is_even (f32)
Indicates even rows, useful for drawing alternate lines.

##### is_folder (f32)
Indicates whether the node is a folder.

##### scale (f32)
Scale factor for the node.

#### States
| State     | Trigger                               |
|-----------|---------------------------------------|
| focussed  | Node has focus                        |
| hover     | User moves the mouse over the element |
| opened    | Node is unfolded                      |
| selected  | Node is selected                      |

### DrawIconQuad ([DrawQuad](DrawQuad.md))
Draws the icon for nodes in the `FileTree`.

#### Flags
##### is_even (f32)
Indicates even rows, useful for drawing alternate lines.

##### is_folder (f32)
Indicates whether the node is a folder.

##### scale (f32)
Scale factor for the node.

#### States
| State     | Trigger                               |
|-----------|---------------------------------------|
| focussed (f32) | Node has focus                        |
| hover (f32)     | User moves the mouse over the element |
| opened (f32)    | Node is unfolded                      |
| selected (f32) | Node is selected |


## Examples

### Typical
```rust
MyFileTree = <FileTree> {
    file_node: <FileTreeNode> {
        is_folder: false,
        draw_bg: { is_folder: 0.0 },
        draw_name: { is_folder: 0.0 }
    }

    folder_node: <FileTreeNode> {
        is_folder: true,
        draw_bg: { is_folder: 1.0 },
        draw_name: { is_folder: 1.0 }
    }

    node_height: 15.0,
    scroll_bars: <ScrollBars> {}

    // LAYOUT PROPERTIES

    height: 500.0,
    // Element is 500.0 high.

    width: Fill,
    // Element expands to use all available horizontal space.
}
```
*This example demonstrates a basic `FileTree` with separate nodes for files and folders.*

### Advanced
```rust
MyFileTree = <FileTree> {
    file_node: <FileTreeNode> {
        is_folder: false,
        draw_bg: { is_folder: 0.0 },
        draw_name: { is_folder: 0.0 },
        check_box: <CheckBox> {},
        indent_shift: 10.0,
        min_drag_distance: 3.0,
        draw_icon: {
            svg_file: dep("crate://self/resources/icons/file.svg"),

            fn get_color(self) -> vec4 {
                return mix(
                    mix(self.color, mix(self.color, #f, 0.5), self.hover),
                    self.color_pressed,
                    self.pressed
                );
            }
        },
        icon_walk: {
            margin: 10.0,
            width: 16.0,
            height: Fit
        }
    }

    folder_node: <FileTreeNode> {
        is_folder: true,
        draw_bg: { is_folder: 1.0 },
        draw_name: { is_folder: 1.0 },
        indent_shift: 10.0,
        min_drag_distance: 3.0,
        draw_icon: {
            svg_file: dep("crate://self/resources/icons/folder.svg"),

            fn get_color(self) -> vec4 {
                return mix(
                    mix(self.color, mix(self.color, #f, 0.5), self.hover),
                    self.color_pressed,
                    self.pressed
                );
            }
        },
        icon_walk: {
            margin: 10.0,
            width: 16.0,
            height: Fit
        }
    }

    node_height: 15.0,
    scroll_bars: <ScrollBars> {},
    draw_scroll_shadow: {},

    // LAYOUT PROPERTIES

    height: Fill,
    width: 250.0,
    margin: 5.0,
    padding: 5.0,
    flow: Down,
    spacing: 2.5,
    align: { x: 0.0, y: 0.0 },
    line_spacing: 1.5
}

<MyFileTree> {}
```
*This advanced example illustrates how to customize the `FileTree` with additional properties and nested widgets.*
