In the previous step, we created a minimal Makepad app. In this step, we're going to create a an initial implementation of an image grid and add it to our app.

Our image grid will be organised as follows:
- Each image is stored in an item.
- Multiple items are laid out horizontally within a row.
- Multiple rows are laid out vertically within a grid.

To keep our initial implementation simple, it will have the following limitations:
- The number of rows will be fixed.
- The number of items per row will be fixed.
- Instead of actual images, we will use a placeholder image.

By the end of this step, your app will display a grid of placeholder images.
## What you will learn
In this step, you will learn:
- How to add resources to your app.
- How to define your own widgets.
- How to override the behaviour of a widget in Rust.
- How to generate the contents of a list in Rust.
## Adding a Placeholder Image
To use a placeholder image, we need to add it as a resource to our app.

To do this, first make sure you're in your package directory, and then run the following:
```
mkdir resources
```

This will create a new directory named `resources`. Navigate to it, and then download the following file to it:
[[placeholder.jpg]]

We'll be using this file as our placeholder image.

**Note:** All files in the `resources` directory are automatically bundled with your app when it is built.
## Updating the DSL Code
Now that we've added a placeholder image, we're going to update the DSL code with the definitions we need. Replace the call to the `live_design` macro in `app.rs` with the one here below:
```
live_design! {
    use link::widgets::*;

    PLACEHOLDER = dep("crate://self/resources/placeholder.jpg");

    ImageItem = <View> {
        width: 256,
        height: 256,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
    }

    ImageRow = {{ImageRow}} {
        <PortalList> {
            height: 256,
            flow: Right,

            ImageItem = <ImageItem> {}
        }
    }

    ImageGrid = {{ImageGrid}} {
        <PortalList> {
            flow: Down,

            ImageRow = <ImageRow> {}
        }
    }

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {
                    <ImageGrid> {}
                }
            }
        }
    }
}
```

There’s quite a bit happening here, so let’s break it down.
### Defining a Variable
The following code defines a variable named `PLACEHOLDER`:
```
    PLACEHOLDER = dep("crate://self/resources/placeholder.jpg");
```

The expression:
```
dep("crate://self/resources/placeholder.jpg")
```

is used to refer to a **dependency**. Dependencies in Makepad are resources that are bundled with your app — in this case, the placeholder image we added to the **resources** directory earlier. 

Having to write this full expression every time we want to refer to the placeholder image would become verbose rather quickly. That's why we define a variable for it here.
### Defining an `ImageItem`:
The following code defines an `ImageItem`:
```
    ImageItem = <View> {
        width: 256,
        height: 256,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
    }
```

An `ImageItem` acts as a container for an image, and any other metadata about the image (such as filename, timestamp, etc) we may want to display later.

Each `ImageItem` has the following properties:
- `width: 256` and `height: 256` ensure the item has a fixed size.

Within each `ImageItem`, we use an `Image` to display the actual image:
```
        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
```

This `Image` has the following properties:
- `width: Fill` and `height: Fill` ensure the image stretches to fill its container.
- `fit: Biggest` ensures the image stretches so that its *biggest* side fills its container, while maintaining its aspect ratio.
- `source: (PLACEHOLDER)` sets our placeholder image as the image source, using the variable we defined earlier.

**Note:** to evaluate an expression in Makepad DSL code, you have to enclose the expression in parentheses `(...)`. The `PLACEHOLDER` variable is an expressions too, so we have to write it as `(PLACEHOLDER)`.

### Defining an `ImageRow`:
The following code defines an `ImageRow`:
```
    ImageRow = {{ImageRow}} {
        <PortalList> {
            height: 256,
            flow: Right,
            
            ImageItem = <ImageItem> {}
        }
    }
```

An `ImageRow` is responsible for laying out multiple `ImageItem`s horizontally.

Within each `ImageRow`, we use a `PortalList` to list its items.
```
        <PortalList> {
            height: 256,
            flow: Right,
            
            ImageItem = <ImageItem> {}
        }
```

**Note:** A `PortalList` is like a standard list but with support for *infinite scrolling*: it can effectively handle large lists by only rendering visible items. We don’t actually need infinite scrolling, but at the time of this writing, Makepad doesn’t have a standard list yet, so we use `PortalList` as a workaround.

This `PortalList` has the following properties:
- `height: 256` ensures the list has enough vertical space for each item.
- `flow: Right` ensures the list's children are laid out from left to right.

Unlike with other widgets, the contents of a `PortalList` are not determined by DSL code, but *must* be generated in Rust code.

Therefore, the following line:
```
            ImageItem = <ImageItem> {}
```

does *not* define an instance of an `ImageItem`, as you might expect. Instead, it defines a **template** for an `ImageItem`. Later on, when we generate the contents of this `PortalList`, we can use this template to create instances of the items we need.

Recall that the `{{ImageRow}}` syntax tells Makepad that the definition of `ImageRow`  corresponds to an `ImageRow` struct in the Rust code. Since the contents of a `PortalList` must be determined in Rust code, we have to use a Rust struct here.
### Defining an ImageGrid
The following code defines an `ImageGrid`:
```
.   ImageGrid = {{ImageGrid}} {
        <PortalList> {
            flow: Down,

            ImageRow = <ImageRow> {}
        }
    }
```

An `ImageGrid` is responsible for laying out multiple `ImageRow`s vertically.

As with `ImageRow`, within each `ImageGrid`, we use a `PortalList` to list its rows. The only difference is that we use `flow: Down`, which ensures its children are laid out from top to bottom.
## Updating the Rust Code
Next, let's update the Rust code with the definitions we need.
### Defining an `ImageRow` struct
Add the following code to `app.rs`:
```
#[derive(Live, LiveHook, Widget)]
pub struct ImageRow {
    #[deref] view: View,
}
```

This code defines an `ImageRow` struct. Recall that earlier we defined an `ImageRow` in the DSL code that corresponds to this struct. By defining a struct for `ImageRow`, we can override its behaviour, such as how it is drawn, and how it responds to events.

Note that we derive several traits for the `ImageRow` struct. We already know what the `Live` and `LiveHook` traits do, but the `Widget` trait is new:
#### Deriving the `Widget` trait
The `Widget` trait is what allows us to override the behaviour of a widget.

Somewhat confusingly, deriving the `Widget` trait does _not_ generate an implementation of it. Instead, it generates implementations of several *helper traits* that the `Widget` traits needs. This makes implementing `Widget` easier, but we still have to write that implementation ourselves. We'll do that in the next section.

The `#[deref]` attribute is used when deriving the `Widget` trait. Putting this attribute on the `view` field of the `ImageRow` struct allows us to use an `ImageRow` as if it were a `View`: deriving the `Widget` trait automatically generates code for  dereferencing an `ImageRow` to a `View`, as well as the appropriate DSL bindings.
### Implementing the `Widget` trait
Add the following code to `app.rs`:
```
impl Widget for ImageRow {
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 4);
                while let Some(item_idx) = list.next_visible_item(cx) {
                    let item = list.item(cx, item_idx, live_id!(ImageItem));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}
```

This code implements the `Widget` trait for the `ImageRow` struct.

The `Widget` trait contains two methods:
- The `handle_event` method controls how an `ImageRow` responds to events. We don't need any custom handling of events for now, so we simply forward all events to the view.
- The `draw_walk` method controls how the widget is drawn. Since the contents of the `PortalList` in ImageRow must be defined in Rust, we *do* need custom drawing, so the implementation of this method is a bit more involved. We'll go over it here below.
#### Drawing each item in a `View`
Let's take a closer look at the `draw_walk` method:
```
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 4);
                while let Some(item_index) = list.next_visible_item(cx) {
                    let item = list.item(cx, item_index, live_id!(ImageItem));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
```

Inside the `draw_walk` method, we first call `self.view.draw_walk(cx, scope, walk)` in a loop to draw each item in the view.

The `draw_walk` function on `view` is a so called **resumable function** — it behaves similarly to an iterator, yielding items one at a time as we step through the drawing process.

On each call to `draw_walk`, it returns a special `DrawStep` object, which represents the current state of the drawing process. We then call the `step` method on this object, which yields the next item that should be drawn. The caller (that's us) is then responsible for drawing each item, allowing us to customise how it is drawn.

Once there are no more items to draw, the call to the `step` method returns `None`, and the loop exits.
#### Drawing a `PortalList`
The following code is responsible for drawing the `PortalList` (recall that within `ImageRow`, we use a `PortalList` to list its items.)
```
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 4);
                while let Some(item_index) = list.next_visible_item(cx) {
                    let item = list.item(cx, item_index, live_id!(ImageItem));
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
```

For each item to be drawn, we first call `as_portal_list` to check if the item is a `PortalList`. Once we have a reference to a `PortalList`:
- We first call `list.set_item_range(cx, 0, 4)` to tell the list we want four items.
- We then call `next_visible_item(cx)` to iterate over the indices of each visible item.
- For each index:
	- We first call `list.item(...)` to get a reference to the item at that index.
	- We then call `item.draw_all(...)` to draw that item to the screen.

Each `PortalList` has its own index namespace — meaning that the indices for each item are unique within each list. The call to `list.item(cx, item_index, live_id!(ImageItem))` checks whether an item already exists for the given index. If it doesn't, it creates an instance of the item.

How does the call to `list.item(cx, item_index, live_id!(ImageItem))` know what to instantiate? Simple: it uses the template for `ImageItem` we defined in the DSL earlier:
```
            ImageItem = <ImageItem> {}
```

**Note:** The `live_id!` macro is used to generate unique identifiers in Makepad. In this case, `live_id!(ImageItem)` refers to the template for `ImageItem`.

### Adding the Rest of the Code
Add the following code to `app.rs`:
```
#[derive(Live, LiveHook, Widget)]
pub struct ImageGrid {
    #[deref] view: View,
}

impl Widget for ImageGrid {
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, 3);
                while let Some(row_idx) = list.next_visible_item(cx) {
                    let row = list.item(cx, row_idx, live_id!(ImageRow));
                    row.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope)
    }
}
```

The rest of the code defines an `ImageGrid` struct and implements the `Widget` trait for it. This code is highly similar to what we wrote for the `ImageRow` struct, so we won't go over it in detail. The only difference is that instead of drawing 4 `ImageItems`, we are drawing 3 `ImageRows`.
## Checking our Progress so far
Let's check our progress so far.

Make sure you’re in your package directory, and run:
```
cargo run --release
```

If everything is working correctly, a window displaying an image grid should appear on your screen:
![[Static Image Grid.png]]
We now have an initial implementation of an image grid. It's still very static — it always displays both the same number of rows and number of items per row, and it always displays the same placeholder image, but we're off to a good start!

In the next step, we'll make the image grid dynamic by loading real images.