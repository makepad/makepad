The `SlidesView` widget provides a container for displaying a sequence of slides with smooth transitions. It allows navigation between slides using arrow keys or programmatically via methods. This widget is ideal for creating presentations or any content that requires sequential display with animations.

## [Layouting](Layouting.md)

Complete layouting feature set support.

## Fields

### anim_speed (f64)

The speed at which slides transition between each other. A lower value results in faster transitions. This value is used in the animation function to interpolate between slides.

## Examples

### Basic Usage

```rust
<SlidesView> {
    <Slide> {
        title = { text: "Welcome" },
        <SlideBody> { text: "This is the opener slide" }
    }
    <Slide> {
        title = { text: "Introduction" },
        <SlideBody> { text: "Overview of the presentation" }
    }
    <Slide> {
        title = { text: "Conclusion" },
        <SlideBody> { text: "Thank you for attending" }
    }

    // LAYOUT PROPERTIES

    width: Fill,
    height: Fill,
}
```

*This example creates a basic slideshow with three slides, filling the available width and height.*

### Advanced Usage

```rust
MySlidesView = <SlidesView> {
    anim_speed: 0.05,
    goal_slide: 2.0,

    <Slide> {
        title = { text: "Overview" },
        <SlideBody> { text: "Starting point of the presentation" }
    }
    <Slide> {
        title = { text: "Details" },
        <SlideBody> { text: "In-depth information" }
    }
    <Slide> {
        title = { text: "Summary" },
        <SlideBody> { text: "Wrapping up the presentation" }
    }

    // LAYOUT PROPERTIES

    width: Fill,
    height: Fill,
    margin: 10.0,
    padding: 20.0,
    align: { x: 0.5, y: 0.5 }
}

<MySlidesView> {}
```

*In this advanced example, a custom `MySlidesView` is defined with adjusted `anim_speed` for faster transitions and `goal_slide` to start at the third slide. Additional layout properties like `margin`, `padding`, and `align` center the slideshow and add spacing.*

## Navigation

Slides can be navigated using the arrow keys by default. To programmatically navigate between slides, you can call the methods `next_slide()` and `prev_slide()` on the `SlidesView` instance.

### Example: Programmatic Navigation

```rust
<MySlidesView> {
    on_click: |cx| {
        self.next_slide(cx);
    }
}
```

*This example advances to the next slide when a click event occurs.*
