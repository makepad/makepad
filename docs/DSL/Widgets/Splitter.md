This widget is used to divide a region into two resizable sections, either horizontally or vertically. It allows users to adjust the size of these sections by dragging a splitter bar.

## Layouting

Support for [Walk](Walk.md) layout features which define how the widget positions itself in its parent container.

No Support for layouting child elements via the feature subset defined in [Layout](Layout.md).

## Splitter

### DrawShaders

#### draw_splitter ([DrawSplitter](#drawsplitter))

Reference to the shader which determines the appearance of the splitter.

### Fields

#### a (WidgetRef)

Reference to the first of the two container widgets to be displayed next to each other. Typically, this would be a `<View>`.

#### b (WidgetRef)

Reference to the second of the two container widgets to be displayed next to each other. Typically, this would be a `<View>`.

#### align ([SplitterAlign](#splitteralign))

Determines the position of the splitter. The `SplitterAlign` enum specifies how the splitter is aligned between the two panels.

##### SplitterAlign

There are three different alignment modes:

- **FromA(f64)**: Fixed position from the start of axis A (left or top). The `f64` value specifies the offset in pixels.
- **FromB(f64)**: Fixed position from the end of axis B (right or bottom). The `f64` value specifies the offset in pixels.
- **Weighted(f64)**: Relative position as a fraction between `0.0` and `1.0`, where `0.0` is at the start (left/top) and `1.0` is at the end (right/bottom). The default is `0.5` (split evenly).

#### axis ([SplitterAxis](#splitteraxis))

Determines the layout orientation of the two panels `A` and `B`.

##### SplitterAxis

The two available placement options:

- **Horizontal**: `A` and `B` are displayed side by side.
- **Vertical**: `A` is displayed above `B`.

#### min_horizontal (f64)

The minimum size in pixels when resizing horizontally.

#### max_horizontal (f64)

The maximum size in pixels when resizing horizontally.

#### min_vertical (f64)

The minimum size in pixels when resizing vertically.

#### max_vertical (f64)

The maximum size in pixels when resizing vertically.

#### split_bar_size (f64)

The width of the splitter bar in pixels.

#### walk ([Walk](Walk.md))

Defines how the splitter itself is laid out within its parent container.

## DrawSplitter

### DrawShaders

#### draw_super ([DrawQuad](DrawQuad.md))

The shader that determines the appearance of the splitter.

##### Flags

- **is_vertical (f32)**: A flag indicating orientation (`1.0` for vertical, `0.0` for horizontal).

## States

| State         | Trigger                                             |
| :------------ | :-------------------------------------------------- |
| hover (f32)   | User moves the mouse over the splitter              |
| pressed (f32) | Mouse button is pressed down on the splitter        |

## Example

### Typical

```Rust
<Splitter> {
    a: <View> { <Label> { text: "Container A" } } // First panel
    b: <View> { <Label> { text: "Container B" } } // Second panel
    align: Weighted(0.5) // Split evenly between the two panels
    axis: Vertical // Panels are arranged vertically (A above B)
}
```

### Advanced 

```Rust
MySplitter = <Splitter> {
    a: <View> { <Label> { text: "Container A" } } // First panel
    b: <View> { <Label> { text: "Container B" } } // Second panel
    align: FromA(100.0) // Splitter is 100 pixels from the top
    axis: Vertical // Panels are arranged vertically

    min_horizontal: 50.0 // Minimum horizontal size when resizing
    max_horizontal: 500.0 // Maximum horizontal size when resizing
    min_vertical: 50.0 // Minimum vertical size when resizing
    max_vertical: 500.0 // Maximum vertical size when resizing
    split_bar_size: 5.0 // Width of the splitter bar

    draw_splitter: {
        instance pressed: 0.0
        instance hover: 0.0

        uniform color: #00000000
        uniform color_hover: #AAAAAA
        uniform color_pressed: #999999

        uniform border_radius: 1.0
        uniform splitter_pad: 1.0
        uniform splitter_grabber: 10.0

        fn pixel(self) -> vec4 {
            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
            sdf.clear(#00000000); // Use a transparent background

            if self.is_vertical > 0.5 {
                sdf.box(
                    self.splitter_pad,
                    self.rect_size.y * 0.5 - self.splitter_grabber * 0.5,
                    self.rect_size.x - 2.0 * self.splitter_pad,
                    self.splitter_grabber,
                    self.border_radius
                );
            } else {
                sdf.box(
                    self.rect_size.x * 0.5 - self.splitter_grabber * 0.5,
                    self.splitter_pad,
                    self.splitter_grabber,
                    self.rect_size.y - 2.0 * self.splitter_pad,
                    self.border_radius
                );
            }
            return sdf.fill_keep(mix(
                self.color,
                mix(
                    self.color_hover,
                    self.color_pressed,
                    self.pressed
                ),
                self.hover
            ));
        }
    }

    animator: {
        hover = {
            default: off
            off = {
                from: { all: Forward { duration: 0.1 } }
                apply: {
                    draw_splitter: { pressed: 0.0, hover: 0.0 }
                }
            }
            on = {
                from: {
                    all: Forward { duration: 0.1 }
                    state_down: Forward { duration: 0.01 }
                }
                apply: {
                    draw_splitter: {
                        pressed: 0.0,
                        hover: [{ time: 0.0, value: 1.0 }],
                    }
                }
            }
            pressed = {
                from: { all: Forward { duration: 0.1 } }
                apply: {
                    draw_splitter: {
                        pressed: [{ time: 0.0, value: 1.0 }],
                        hover: 1.0,
                    }
                }
            }
        }
    }

    // LAYOUT PROPERTIES
    height: Fill,
    width: Fill,
    margin: { top: 10.0, right: 0.0, bottom: 10.0, left: 0.0 },
}

<MySplitter> {} // Instantiate the custom splitter
```