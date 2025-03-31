The `FoldHeader` widget is a composite widget that provides expandable and collapsible sections within the UI. It consists of a header that is always visible and a body that can be toggled open or closed, typically controlled by a `FoldButton` within the header.

## Layouting
Complete layouting feature set support.

## Fields

### header (WidgetRef)

  Reference to the widget that makes up the persistent header of the `FoldHeader`. This typically includes the `FoldButton` and any header content.

### body (WidgetRef)

  Reference to the widget containing the content to be toggled. This is usually a `<View>` with child elements that you want to show or hide.


### body_walk ([Walk](Walk.md))
Controls the body widget's layout properties, such as size and positioning, as supported by the [`Walk`](Walk.md) layout system.


## States

| State        | Trigger                                                 |
|--------------|---------------------------------------------------------|
| `opened` (f32) | When the `FoldButton` is clicked to open or close the body content |

## Examples

### Basic Usage

```Rust
<FoldHeader> {
    header: <View> {
        fold_button = <FoldButton> {}
        <Label> { text: "Fold me!" }
        width: Fill, height: Fit
    }

    body: <View> {
        <Label> { text: "This is the body that can be folded away" }
        width: Fill, height: Fit
    }

    width: Fill,
    height: Fill,
}
```
*In this basic example, the `FoldHeader` contains a header with a `FoldButton` and a label. The body includes content that can be toggled open or closed.*

### Typical

```Rust
MyFoldHeader = <FoldHeader> {
    header: <View> {
        height: Fit,
        align: { x: 0.0, y: 0.5 },
        fold_button = <FoldButton> {}
        <Label> { text: "Fold me!" }
    }

    body: <View> {
        width: Fill, height: Fit,
        padding: 5.0,
        <Label> { text: "This is the body that can be folded away" }
    }

    body_walk: { width: Fill, height: Fit },

    // LAYOUT PROPERTIES

    width: Fill,
    // The widget expands to use all available horizontal space.

    height: Fill,
    // The widget expands to use all available vertical space.

    flow: Down,
    // Stacks children vertically from top to bottom.

    spacing: 10.0,
    // Sets a spacing of 10.0 between the header and body.
}
```
*This typical example demonstrates additional layout properties and padding for the body content, providing a more refined UI component.*

### Advanced

```Rust
MyFoldHeader = <FoldHeader> {
    header: <View> {
        height: Fit,
        align: { x: 0.0, y: 0.5 },
        fold_button = <FoldButton> {}
        <Label> { text: "Fold me!" }
    }

    body: <View> {
        width: Fill, height: Fit,
        show_bg: false,
        padding: 5.0,
        <Label> { text: "This is the body that can be folded away" }
    }

    body_walk: { width: Fill, height: Fit },

    animator: {
        open = {
            default: on,
            off = {
                from: { all: Forward { duration: 0.2 } },
                ease: ExpDecay { d1: 0.96, d2: 0.97 },
                redraw: true,
                apply: {
                    opened: [{ time: 0.0, value: 1.0 }, { time: 1.0, value: 0.0 }]
                }
            },
            on = {
                from: { all: Forward { duration: 0.2 } },
                ease: ExpDecay { d1: 0.98, d2: 0.95 },
                redraw: true,
                apply: {
                    opened: [{ time: 0.0, value: 0.0 }, { time: 1.0, value: 1.0 }]
                }
            }
        }
    },

    // LAYOUT PROPERTIES

    width: Fill,
    height: Fit,
    margin: 5.0,
    padding: 5.0,
    flow: Down,
    spacing: 2.5,
    align: { x: 0.0, y: 0.5 },
}

<MyFoldHeader> {}
```
*The advanced example includes custom animations for opening and closing, as well as additional layout adjustments, margins, and padding to fine-tune the appearance and behavior of the `FoldHeader`.*

---

By utilizing the `FoldHeader`, you can create interactive UI components that enhance the organization and user experience of your application by allowing sections of content to be expanded or collapsed as needed.
