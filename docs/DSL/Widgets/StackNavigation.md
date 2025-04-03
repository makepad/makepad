The `StackNavigation` control manages a stack of views for navigation purposes, allowing smooth transitions between different views or screens within the application.

## Inherits

- **view**: [[View]]

## Layouting

No layouting support.

## Fields

### offset (f64)

Controls the horizontal position of the navigation view. Used to animate transitions between different views, enabling smooth sliding effects when navigating.

## Subwidgets

### view ([View](View.md))

Contains the child `<View>` widgets representing the different pages or screens within the stack. Each child `<View>` is a navigable page managed by the `StackNavigation` widget.

## States

| State      | Trigger                               |
|------------|---------------------------------------|
| hide (f32) | Page is hidden                        |
| show (f32) | Page is displayed when activated      |

## Examples

### Typical

```rust
MyStackNavigation = <StackNavigationView> {
    // Background view (optional)
    background = <View> {
        width: Fill,
        height: Fill,
        visible: false,
    },

    // Body of the stack navigation
    body = <View> {
        width: Fill,
        height: Fill,
        flow: Down,

        // Adjust margin between body and header
        margin: { top: 20.0 },
    },

    // Header of the stack navigation
    header = <StackViewHeader> {},

    animator: {
        slide = {
            default: hide,
            hide = {
                redraw: true,
                ease: ExpDecay { d1: 0.80, d2: 0.97 },
                from: { all: Forward { duration: 5.0 } },
                // Large enough number to cover several screens
                apply: { offset: 4000.0 },
            },

            show = {
                redraw: true,
                ease: ExpDecay { d1: 0.82, d2: 0.95 },
                from: { all: Forward { duration: 0.5 } },
                apply: { offset: 0.0 },
            },
        }
    },

    // LAYOUT PROPERTIES

    height: Fill,
    // Element expands to use all available vertical space.

    width: Fill,
    // Element expands to use all available horizontal space.
}
```

Defines a basic stack navigation view with a header and body, including animation states for showing and hiding the view.

### Advanced

```rust
MyStackNavigation = <StackNavigationView> {
    offset: 4000.0,
    flow: Overlay,
    visible: false,

    show_bg: true,
    draw_bg: {
        color: #FFFFFF,
    },

    // Background view (optional)
    background = <View> {
        width: Fill,
        height: Fill,
        visible: false,
    },

    // Body of the stack navigation
    body = <View> {
        width: Fill,
        height: Fill,
        flow: Down,

        // Adjust margin between body and header
        margin: { top: 20.0 },
    },

    // Header of the stack navigation
    header = <StackViewHeader> {},

    animator: {
        slide = {
            default: hide,
            hide = {
                redraw: true,
                ease: ExpDecay { d1: 0.80, d2: 0.97 },
                from: { all: Forward { duration: 5.0 } },
                // Large enough number to cover several screens
                apply: { offset: 4000.0 },
            },

            show = {
                redraw: true,
                ease: ExpDecay { d1: 0.82, d2: 0.95 },
                from: { all: Forward { duration: 0.5 } },
                apply: { offset: 0.0 },
            },
        }
    },

    // LAYOUT PROPERTIES

    height: Fill,
    // Element expands to use all available vertical space.

    width: Fill,
    // Element expands to use all available horizontal space.

    margin: 5.0,
    // Margin around the element.

    padding: 5.0,
    // Padding inside the element.

    spacing: 5.0,
    // Spacing between child elements.

    align: { x: 0.0, y: 0.0 },
    // Aligns children to the top-left corner.
}

<MyStackNavigation> {}
```

An advanced stack navigation view with customized flow, visibility, background color, and layout properties.
