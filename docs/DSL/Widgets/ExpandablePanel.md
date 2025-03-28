A widget that creates a panel which can expand and contract based on touch gestures, specifically swipe gestures.

## Inherits
- [[View]]

## [Layouting](Layouting.md)
Complete layouting feature set support.

## Fields

### initial_offset (f64)
The initial vertical offset of the panel, specifying the starting position of the panel relative to the body.

## Examples

### Basic

```rust
<ExpandablePanel> {
    body = <MyBodyView> {}
    panel = <MyPanelView> {}

    // LAYOUT PROPERTIES
    height: Fill,
    width: Fill,
    flow: Down
}
```
*Creates an `ExpandablePanel` with custom body and panel views, filling all available space and stacking elements vertically.*

### Typical

```rust
<ExpandablePanel> {
    body = <View> {}

    panel = <View> {
        flow: Down,
        width: Fill,
        height: Fit,

        scroll_handler = <RoundedView> {
            width: 40.0,
            height: 6.0,

            show_bg: true,
            draw_bg: {
                color: #333,
                radius: 2.0
            }
        }
    }

    // LAYOUT PROPERTIES
    height: Fill,  // Expand to fill all available vertical space
    width: Fill,   // Expand to fill all available horizontal space
    flow: Down     // Stack children vertically
}
```
*An `ExpandablePanel` where the `panel` contains a `scroll_handler` for user interactions. The panel adjusts its height based on content, and the layout properties ensure it fills the available space.*

### Advanced

```rust
MyExpandablePanel = <ExpandablePanel> {
    initial_offset: 400.0,

    body = <View> {}

    panel = <View> {
        flow: Down,
        width: Fill,
        height: Fit,

        show_bg: true,
        draw_bg: { color: #FFF },

        align: { x: 0.5, y: 0 },
        padding: 20.0,
        spacing: 10.0,

        scroll_handler = <RoundedView> {
            width: 40.0,
            height: 6.0,

            show_bg: true,
            draw_bg: {
                color: #333,
                radius: 2.0
            }
        }
    }

    // LAYOUT PROPERTIES
    height: Fit,
    width: Fit,
    margin: 10.0,
    padding: 5.0,
    flow: Overlay,
    spacing: 5.0,
    align: { x: 0.5, y: 0.5 },
    line_spacing: 1.25
}

<MyExpandablePanel> {}
```
*Defines a custom `MyExpandablePanel` with an increased `initial_offset`, customized `panel` appearance, and specific layout properties for fine-tuned control over positioning and spacing.*
