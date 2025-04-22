In the previous two steps, we built an image grid. Image grids allow you to view multiple images at the same time. Sometimes, however, instead of viewing multiple images at the same time, you want to view multiple images one at a time. That's what slideshows are for.

In this step, we're going to create a simple slideshow for our app. Our slideshow will have the following features:
- It will display a single image at full size.
- It will have two buttons on each side, for navigating it by mouse.
- It will be navigable by keyboard.

At the end of this step, you will have a working implementation of a slideshow.
## Adding Arrow Icons
For the two buttons, we need some arrow icons, so let's add those as resources to our app.

Navigate to the `resources` directory, and then download the following files to it:
[[left_arrow.svg]]
[[right_arrow.svg]]

We'll be using these file as our arrow icons.
## Updating the DSL Code
TODO
### Defining Variables
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    LEFT_ARROW = dep("crate://self/resources/left_arrow.svg");
    RIGHT_ARROW = dep("crate://self/resources/right_arrow.svg");
```

This code defines variable named `LEFT_ARROW` and `RIGHT_ARROW` to refer to the arrow icons that we added to the `resource` directory earlier, just as we did for the placeholder image in step 2.
### Defining a `SlideshowButton`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    SlideshowButton = <Button> {
        width: 50,
        height: Fill,
        grab_key_focus: false,
        draw_bg: {
            color: #fff0,
            color_down: #fff2,
        }
        icon_walk: { width: 9 },
        text: ""
    }
```

This code defines a `SlideshowButton`.

This `SlideshowButton` has the following properties:
- `width: 50` and `height: Fill` ensures that each button is a tall, narrow strip, that fills the height of its container.
- `draw_bg { ... }` controls
- `icon_walk { ... }` controls
#### A Primer on Inheritance
You may have noticed that our definition of SlideshowButton looks like this:
```
    SlideshowButton = <Button> {
        ...
    }
```

That syntax means that `SlideshowButton` *inherits* from `Button`.

Inheritance in Makepad works similar to prototypal inheritance in JavaScript.
- The syntax `{ ... }` is used to define an object. An object is simply a collection of properties, each of which has a name and a value.
- The syntax `Object = { ... }` is used to assign a name to an object.
- The syntax `<Base> { ... }` is used to define an object that inherits from an object `Base`.
- When an object inherits from another object, it copies over all properties from that object.
- Objects can override existing properties to change their values.
- Objects can also add new properties that weren't present in the original.

### Defining a `SlideshowOverlay`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    SlideshowOverlay = <View> {
        height: Fill,
        width: Fill,
        cursor: Arrow,
        capture_overload: true,

        left = <SlideshowButton> {
            draw_icon: { svg_file: (LEFT_ARROW) }
        }
        <Filler> {}
        right = <SlideshowButton> {
            draw_icon: { svg_file: (RIGHT_ARROW) }
        }
    }
```

This code defines a `SlideshowOverlay`. A `SlideshowOverlay` is a transparent container that sits on top of an `Image`, and contains two `SlideshowButton`s.

This `SlideshowOverlay` has the following properties:
- `height: Fill` and `width: Fill` ensure the overlay stretches to fill its container.
- `cursor: Arrow` sets the icon of the mouse cursor to an arrow when it hovers over the overlay.
- `capture_overload: true` enables the overlay to capture events that have already been captured by one of its children.

Looking ahead a bit, we want `SlideshowOverlay` to be the widget responsible for navigating the slideshow by keyboard. Note that `SlideshowOverlay` inherits from `View`. The reason we set `cursor: Arrow`, is that without setting a cursor, a `View` will not respond to keyboard events.

Moreover, a `View` only responds to keyboard events when it has keyboard focus. A `View` gets keyboard focus when it is clicked. The reason we set `capture_overload: true`: is that otherwise, a `View` won't respond to click events (and thus won't get keyboard focus) when one of it's children (in this case, one of the `SlideshowButton`s) is clicked instead.

Each `SlideshowOverlay` contains two `SlideshowButton`s, with a `Filler` in between:
```
        left = <SlideshowButton> {
            draw_icon: { svg_file: (LEFT_ARROW) }
        }
        <Filler> {}
        right = <SlideshowButton> {
            draw_icon: { svg_file: (RIGHT_ARROW) }
        }
```

`Filler` is a helper widget that fills up any unused space in a container. This ensures that the first `SlideshowButton` is laid out on the left while the second is laid out on the right.
### Defining a `Slideshow`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    Slideshow = <View> {
        flow: Overlay,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
        overlay = <SlideshowOverlay> {}
    }
```

This code defines a `Slideshow`. A `Slideshow` combines an `Image` with the `SlideshowOverlay` we created earlier.

This `Slideshow` has the following properties:
- `flow: Overlay` ensures the slideshow's children are stacked on top of each other.
### Updating `App`
TODO
```
    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {
                    // <ImageGrid> {}
                    slideshow = <Slideshow> {}
                }
            }
        }
    }
```