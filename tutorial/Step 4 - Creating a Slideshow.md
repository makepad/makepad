In the previous two steps, we built an image grid. Image grids allow you to view multiple images at the same time. Sometimes, however, instead of viewing multiple images at the same time, you want to view multiple images one at a time. That's what slideshows are for.

In this step, we're going to create a simple slideshow for our app. Our slideshow will have the following features:
- It will display a single image at full size.
- It will have two buttons on each side, used for navigating it by mouse.
- It will be navigable by keyboard.

At the end of this step, you will have a working implementation of a slideshow.
## What you will learn
In this step, you will learn:
- How inheritance works in Makepad.
- How key focus works in Makepad.
- How the event-action flow works in Makepad.
- How to respond to user actions.
## Adding Arrow Icons
For the two buttons, we need some arrow icons, so let's start by adding those as resources to our app.

Navigate to the `resources` directory, and then download the following files to it:
[[left_arrow.svg]]
[[right_arrow.svg]]

We'll be using these file as our arrow icons.
## Updating the DSL Code
Now that we've added the arrow icons, let's update the DSL code with the definitions we need.
### Defining Variables
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    LEFT_ARROW = dep("crate://self/resources/left_arrow.svg");
    RIGHT_ARROW = dep("crate://self/resources/right_arrow.svg");
```

This code defines variables named `LEFT_ARROW` and `RIGHT_ARROW` to refer to the arrow icons that we added to the `resource` directory earlier.
### Defining a `SlideshowNavigationButton`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    SlideshowNavigationButton = <Button> {
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

This code defines a `SlideshowNavigationButton`. A `SlideshowNavigationButton` is a tall, narrow strip that takes up the height of its container and contains a single arrow icon. We'll use two `SlideshowNavigationButton`s to navigate the slideshow by mouse.

This `SlideshowNavigationButton` has the following properties:
- `width: 50` and `height: Fill` ensure the button has the desired size.
- `grab_key_focus: false` prevents the button from grabbing key focus when it is clicked (we'll explain this further when we talk about how key focus works).
- `draw_bg { ... }` controls how the button's background is drawn.
	- `color: #fff0` makes the button fully invisible by default.
	- `color_down: #fff2` makes the button slightly more visible when it is pressed.
- `icon_walk { ... }` controls how to button's icon is laid out.
	- `width: 9` makes the icon 9 pixels wide.
- `text: ""` disables the label for this button.
#### A Primer on Inheritance
This would be a good time to talk a bit about how **inheritance** works in Makepad.

Inheritance in Makepad works very similar to prototypal inheritance in languages such as JavaScript:
- The syntax `{ ... }` is used to define an object. An object is simply a collection of properties, each of which has a name and a value.
- The syntax `Object = { ... }` is used to assign a name to an object.
- Top-level named objects can be used as base classes for other objects.
- The syntax `<Base> { ... }` is used to define an object that inherits from an object `Base`.
- When an object inherits from another object, it copies over all properties from that object.
- Objects can override existing properties to change their values.
- Objects can also add new properties that weren't present in the original.

An example of this is is the `SlideshowNavigationButton` we just defined. The definition of `SlideshowNavigationButton` looks like this:
```
    SlideshowNavigationButton = <Button> {
        ...
    }
```

That means `SlideshowNavigationButton` derives from `Button`. Recall that Button is one of the built-in widgets we imported with `use link::widgets::*;`. `SlideshowNavigationButton` copies over all properties from `Button`, and then overrides several of its properties.

You may have noticed that we did not specify an image for the icon in our definition of `SlideshowNavigationButton`. That is because `SlideshowNavigationButton` is *itself* intended to be used as a base class: each time we create an instance of it, we'll specify an image for the icon of that specific instance. You'll see an example of this in `SlideshowOverlay`, just below.
### Defining a `SlideshowOverlay`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    SlideshowOverlay = <View> {
        height: Fill,
        width: Fill,
        cursor: Arrow,
        capture_overload: true,

        navigate_left = <SlideshowNavigationButton> {
            draw_icon: { svg_file: (LEFT_ARROW) }
        }
        <Filler> {}
        navigate_right = <SlideshowNavigationButton> {
            draw_icon: { svg_file: (RIGHT_ARROW) }
        }
    }
```

This code defines a `SlideshowOverlay`. A `SlideshowOverlay` is a transparent container that sits on top of an `Image`, and contains the two `SlideshowNavigationButton`s we'll use for navigating the slideshow by mouse.

This `SlideshowOverlay` has the following properties:
- `height: Fill` and `width: Fill` ensure the overlay stretches to fill its container.
- `cursor: Arrow` sets the icon of the mouse cursor to an arrow when it hovers over the overlay.
- `capture_overload: true` allows the overlay to capture events that have already been captured by one of its children (we'll explain this further when we talk about how key focus works).

Each `SlideshowOverlay` contains two `SlideshowNavigationButton`s, with a `Filler` in between:
```
        navigate_left = <SlideshowNavigationButton> {
            draw_icon: { svg_file: (LEFT_ARROW) }
        }
        <Filler> {}
        navigate_right = <SlideshowNavigationButton> {
            draw_icon: { svg_file: (RIGHT_ARROW) }
        }
```

For each `SlideshowNavigationButton`, we override the `svg_file` property of `draw_icon` with the variables for the arrow icons we defined earlier.

`Filler` is a helper widget that fills up any unused space in a container. This ensures that the first `SlideshowNavigationButton` is laid out on the left while the second is laid out on the right.
### Handling Key Focus
We want `SlideshowOverlay` to handle keyboard input for navigating the slideshow, so this would be a good time to talk about how **key focus** works.

In Makepad, a widget only responds to keyboard events if it has key focus — meaning it’s the active widget for receiving keyboard input. Most built-in widgets, like `View`, don’t respond to key presses unless they’ve been given key focus.

Most built-in widgets automatically grabs key focus when they are clicked — but only when the widget itself is clicked directly. If one of its children is clicked instead, then that child grabs the key focus, not the widget itself.

For `SlideshowOverlay`, that's *not* the behaviour we want: we want `SlideshowOverlay` to grab key focus, even when one of the `SlideshowNavigationButton`s  is clicked. That's why we added the following properties earlier:

- `grab_key_focus: false` on `SlideshowNavigationButton` prevents the button from grabbing key focus when it is clicked  — allowing the overlay to grab it instead.
- `capture_overload: true` on `SlideshowOverlay` allows the overlay to capture events (and thus grab key focus) that have already been captured by one of its children.

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
Replace the definition of `App` in the call to the `live_design` macro with the one here below:
```
    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = {
	                <View> {
	                    // <ImageGrid> {}
	                    slideshow = <Slideshow> {}
	                }
	            }
            }
        }
        placeholder: (PLACEHOLDER)
    }
```

We've commented out our `ImageGrid`, and replaced it with our `Slideshow`. This is just a temporary measure so that we can develop our slideshow without worrying about the image grid. In the next step, we'll make it so we can switch between the image grid and the slideshow at will.

We've also added a `placeholder` field, storing a reference to our placeholder image. This will allow us to refer to the placeholder image from Rust code. For this to work, we need to make sure to also add a placeholder field to the corresponding `App` struct:

```
#[derive(Live)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[live]
    placeholder: LiveDependency,
    #[rust]
    state: State,
}
```
## Extending the State
Now that we've updated the DSL code with the definitions we need, it's time to extend the state for our app with some additional fields and methods we need to make the slideshow dynamic.

Specifically, we’ll add a field to track which image is currently displayed in the slideshow, and a few helper methods for updating that state at runtime. We'll also need to update some of our existing helper methods.
### Updating the `State` struct
Replace the definition of the `State` struct and its corresponding implementation of the `Default` trait with the one here below:
```
#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    images_per_row: usize,
    current_image_idx: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            images_per_row: 4,
            current_image_idx: None,
        }
    }
}
```

As you can see, we've added one additional field to the `State` struct:
- `current_image_idx` contains the index of the image that is currently displayed in the slideshow.

Note that `current_image_idx` is actually an `Option<usize>`. This is to handle the case where there are no images to display:
- If `current_image_idx` is `Some(image_idx)`, the slideshow should display the image with the given index.
- If `current_image_idx` is `None`, the slideshow should display the placeholder image instead.
### Adding Helper Methods
To facilitate updating the slideshow at runtime, we're going to add a few helper methods to the `App` struct.

#### Adding the `set_current_image` method
The `set_current_image` method is used to change which image is currently displayed in the slideshow:
```
impl App {
    pub fn set_current_image(&mut self, cx: &mut Cx, image_idx: Option<usize>) {
        self.state.current_image_idx = image_idx;
        let image = self.ui.image(id!(slideshow.image));
        if let Some(image_idx) = self.state.current_image_idx {
            let image_path = &self.state.image_paths[image_idx];
            image.load_image_file_by_path(cx, &image_path).unwrap();
        } else {
            image
                .load_image_dep_by_path(cx, self.placeholder.as_str())
                .unwrap();
        }
        self.ui.view(id!(slideshow)).redraw(cx);
    }
}
```

Here's what the `set_current_image` method does:
- It sets `current_image_idx` to the new value.
- It gets a reference to the `Image` inside the `Slideshow`.
- If current_image_idx is Some(image_idx):
	- It obtains the corresponding path for this image.
	- It reloads the `Image` using this path.
- Otherwise:
	- It reloads the `Image` with the placeholder image.
- It calls `redraw(..)` on the `Slideshow` to schedule it to be redrawn with the new image.

#### Adding the `navigate_left/navigate_right` methods
Now that we have a method to change which image is currently displayed in the slideshow, we'll add two more helper methods: `navigate_left` and `navigate_right`. These are used to navigate the slideshow to the previous and next image, respectively:

```
impl App {
.   pub fn navigate_left(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx > 0 {
                self.set_current_image(cx, Some(image_idx - 1));
            }
        }
    }

    pub fn navigate_right(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx + 1 < self.state.image_paths.len() {
                self.set_current_image(cx, Some(image_idx + 1));
            }
        }
    }
}
```

Here's what these methods do:
- `navigate_left` first decrements `current_image_idx` by 1, unless we're already at the first image.
- `navigate_right` first increments `current_image_idx` by 1, unless we're already at the last image.
- Both methods then call `set_current_image` to apply the change, and schedule the slideshow to be redrawn.
### Updating the `update_image_paths` method
Replace the definition of the `update_image_paths` method on `App` with the one here below:
```
impl App {
    pub fn update_image_paths(&mut self, cx: &mut Cx, path: &Path) {
        self.state.image_paths.clear();
        for entry in path.read_dir().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            self.state.image_paths.push(path);
        }
        if self.state.image_paths.is_empty() {
            self.set_current_image(cx, None);
        } else {
            self.set_current_image(cx, Some(0));
        }
    }
}
```

The only thing that's changed here is the 4 new lines at the end:
```
        if self.state.image_paths.is_empty() {
            self.set_current_image(cx, None);
        } else {
            self.set_current_image(cx, Some(0));
        }
```

Here's what this code does:
- If the list of images is empty, it sets the current image to None.
- Otherwise, it sets the current image to the first image in the list.

This change ensures that the slideshow is always in a valid state, even if the list of images changes.
## Responding to User Actions
We just added several helper methods to facilitate updating the slideshow at runtime. Our next step is to wire things up so that these methods get called whenever the user interacts with the UI — either by clicking one of the buttons in the slideshow, or by the pressing the left/right arrow key on the keyboard. To do this, we're going to use something called actions. 

An **action** is a notification that something interesting happened to a *widget*. For instance, the `Button` widget has a `clicked` action that it dispatches to notify the rest of the app when it has been clicked.

In contrast, an **event** in Makepad is a notification that the *user* did something interesting. For instance, when the user clicks the mouse button, its causes a mouse-down event to be dispatched to the app.

### The Event-Action Flow
Let's take a look at how the event-action flow works in Makepad:
- An event is dispatched to the app to notify it app that the user did something interesting.
- The event **bubbles down** from the root of the app to the bottom of the widget tree.
- When the event reaches a widget that is prepared to handle it, that widget:
	- *Will* update itself in response to the event.
	- *May* dispatch an action to notify the rest of the app that something interesting happened to it.
- The action **bubbles up** from the widget to the root of the app.
- Widgets higher up in the tree can listen to and respond to these actions.

This pattern helps to separate low-level input handling from high-level UI behaviour.
### Handling Actions
To handle actions in our app, we can use the `MatchEvent` trait.

The `MatchEvent` trait provides several overridable methods that will be called for specific events. When you call the `match_event` method on the `MatchEvent` trait with an event, it automatically forwards that event to the appropriate method.

We'll start by adding a call to `match_event` to the `handle_event` method on the `App` struct:
```
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}
```

Next, we'll implement the `MatchEvent` trait for the `App` struct. For our use case, the method we are interested in is `handle_actions`. Here's our implementation:

```
impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(id!(navigate_left)).clicked(&actions) {
            self.navigate_left(cx);
        }
        if self.ui.button(id!(navigate_right)).clicked(&actions) {
            self.navigate_right(cx);
        }
        if let Some(event) =
            self.ui.view(id!(slideshow.overlay)).key_down(&actions)
        {
            match event.key_code {
                KeyCode::ArrowLeft => self.navigate_left(cx),
                KeyCode::ArrowRight => self.navigate_right(cx),
                _ => {}
            }
        }
    }
}
```

Let's look at what this code does in more detail.
#### Handling Button Clicks
The following code:
```
        if self.ui.button(id!(navigate_left)).clicked(&actions) {
            self.navigate_left(cx);
        }
        if self.ui.button(id!(navigate_right)).clicked(&actions) {
            self.navigate_right(cx);
        }
```

checks whether one of the buttons in the slideshow were clicked. If so, it calls the appropriate helper method (either `navigate_left` or `navigate_right`) to update the slideshow.

**Note:**
Whenever one or more actions are dispatched by a widget, an **action event** is dispatched containing the list of dispatched actions. When the `match_event` method on the `MatchEvent` is trait called with an action event, it causes the `handle_actions` method to be called with the list of dispatched actions. To handle actions, each widget provides one or more helper methods such as the `clicked` method on Button above. When you call this method with a list of  actions, it checks if one of the actions signified a button click, and if so, returns `true`.
#### Handling Key Presses
The following code:
```
        if let Some(event) =
            self.ui.view(id!(slideshow.overlay)).key_down(&actions)
        {
            match event.key_code {
                KeyCode::ArrowLeft => self.navigate_left(cx),
                KeyCode::ArrowRight => self.navigate_right(cx),
                _ => {}
            }
        }
```

checks whether the left or right arrow key was pressed while the slideshow overlay has key focus. If so, it calls the appropriate helper method (either `navigate_left` or `navigate_right`) to update the slideshow.
## Checking our Progress so far
Let's check our progress so far.

Make sure you’re in your package directory, and run:
```
cargo run --release -- path/to/your/images
```

If everything is working correctly, a slideshow should now appear in your window:
![[Slideshow 1.png]]
Clicking the buttons on the left/right should navigate the slideshow to the previous/next image, respectively. Alternatively, pressing the left/right arrow keys on the keyboard should have the same effect:
![[Slideshow 2.png]]
We now have a working implementation of a slideshow. In the next step, we'll enable switching between the image grid and the slideshow.

**Note:** For keyboard input to work, you need to make sure that the slideshow overlay has keyboard focus. To do this, click anywhere on the slideshow first.