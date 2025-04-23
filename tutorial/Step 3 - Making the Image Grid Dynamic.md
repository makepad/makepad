In the previous step, we created an initial implementation image grid, and added it to our app.

To keep our initial implementation simple, it had the following limitations:
- The number of rows was fixed.
- The number of items per row was fixed.
- Instead of actual images, we used a placeholder image.

In this step, we're going to make the image grid dynamic:
- Instead of a placeholder image, we will use real images.
- The number of rows and the number items per row will depend on the number of images.

By the end of this step, your app will display a grid of real images.
## What you will learn
In this step, you will learn:
- How to add state to your app.
- How to initialise that state on startup.
- How to use that state in your widgets.
## Downloading the Images
Since we're going to display real images, we first need some images to display. You can either use your own images, or download an archive of the images we've used here:
![[images.zip]]
## Adding State
To make our image grid dynamic, we need a place to store the state we need — like which images to show, and how to lay them out.
### Defining a `State` struct
To do this, first add the following code to `app.rs`:
```
#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    images_per_row: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            images_per_row: 4,
        }
    }
}
```

This code defines a `State` struct and implements the `Default` trait for it. The `State` struct stores any state that our app needs.

At the moment, the `State` struct contains only two fields:
- `image_paths` contains the paths of the images we want to show.
- `images_per_row` contains the number of images for each row in the grid.

This is all the information we need for now to draw our image grid.
### Updating the `App` struct
Next, replace the definition of the `App` struct in `app.rs` with the one here below:

```
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] state: State,
}
```

This adds a `state` field to the `App` struct. This field contains an instance of the `State` struct we just defined.

Note that we used the `#[rust]` attribute to mark the `state` field as an ordinary Rust field. Recall that when the live design systems encounters a field marked with the `#[rust]` attribute, it uses the `Default::default` constructor for values of that type to instantiate the field. This is why we implemented the `Default` trait for the `State` struct earlier.
## Initialising the State
Now that we have a place to store the state for our app, we're going to initialise it with actual data. Specifically, we're going to populate the `image_paths` field with the paths to the files in a directory.

To do this, add the following code to `app.rs`:
```
impl App {
    pub fn update_image_paths(&mut self, path: &Path) {
        self.state.image_paths.clear();
        
        for entry in path.read_dir().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();

            if !path.is_file() {
                continue;
            }
            
            self.state.image_paths.push(path);
        }
    }
}
```

This defines an `update_image_paths` method on the `App` struct. Let's quickly take a look at what this method does:
- First, it removes any existing paths from `image_paths`.
- Next, it iterates over all entries in the directory at the given `path`:
- For each entry:
	- It checks whether the entry is a file.
	- If it is not, the entry is skipped.
	- Otherwise, the entry's path is added to `image_paths`.

This effectively replaces the paths in `image_paths` with the paths to the files in the directory at the given `path`.

For simplicity, we have not added any error handling to the `update_image_paths` method. There are several ways that this method could fail, including:
- The given `path` does not exist.
- The given `path` exists, but points to a non-directory file.
- We don't have permission to read the directory.

If any of these errors occur, our app will simply panic. That is acceptable for a tutorial, but in a real life app, we'd want more robust error handling here.

We now have a method to populate the `image_paths` field with the paths to the files in a directory, but this method is not being called yet. We'll take care of that next.

## Running Initialisation at Startup
We need to make sure that the `update_image_paths` method we defined earlier is called when the application starts.

To do this, we can use the `LiveHook` trait. Recall that this trait provides several overridable methods which will be called at various points during our app's lifetime.
### Updating the App Struct
First, replace the definition of the `App` struct in `app.rs` with the one here below:
```
#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] state: State,
}
```

This removes the derivation of the `LiveHook` trait from the App struct.  Recall that deriving the `LiveHook` trait for a struct generates an empty implementation of it for that struct. Since we're about to write our own implementation of the `LiveHook` trait for the `App` struct, we need to get rid of the generated version. Otherwise, the Rust compiler will complain about the `LiveHook` trait being implemented twice.
### Implementing the `LiveHook` trait
Next, add the following code to `app.rs`:
```
impl LiveHook for App {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
        self.load_image_paths(env::args().nth(1).unwrap().as_ref())
    }
}
```

This code implements the `LiveHook` trait for the `App` struct. We've overridden a single method, `after_new_from_doc`. This method will be called after our app has been fully initialised by the live design system, but before it starts running. That makes it the perfect place to call the `update_image_paths` method.

Let's quickly take a look at what our implementation of the `after_new_from_doc` method on `LiveHook` does:
- First, it obtains the first command-line argument, by calling `env::args::nth(1)`.
- Next, it converts this command-line argument to a path, by calling `as_ref()`.
- Finally, it calls `update_image_paths` with this path.

We use a command-line argument here because it's the simplest way to pass a path to our app. This makes it easy to test our app while we're building the core functionality.

## Using the State
Now that we have a place to store the state for our app, and a way to initialise it with actual data, let's take a look at how we can *use* this state in our app.
### Passing the State to our Widgets
Our immediate problem is that our state lives on the `App` struct, but our widgets have no way to access that state. In order to to make our state accessible to our widgets, we need to pass it to the widget tree.

Let's take a look at our current implementation of the `AppMain` trait for the `App` struct:
```
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
```

Note how we are calling `Scope::empty` to create an empty scope for each event. Recall that a scope in Makepad is a container that is used to pass both app-wide **data** and widget-specific **props** along with each event. Data is typically mutable, while props are immutable.

Up until now, we didn't have any state yet, so we simply created an empty scope for each event. Now that we have some actual state, it's time to change this.

Replace the implementation of the `AppMain` trait for the `App` struct with the one here below:
```
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}
```

This uses `Scope::with_data` to create a new scope, containing a mutable reference to our `State`, and passes it to the root of our widget tree. From there, it will automatically be forwarded to `ImageGrid`, where we can access it, and then further forward it to `ImageRow`.
### Using the State in `ImageGrid`
Now that we can access our state in `ImageGrid`, let's take a look at how it is used.

Replace the implementation of the `draw_walk` method for `ImageGrid` with the one here below:
```
impl Widget for ImageGrid {
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let state = scope.data.get_mut::<State>().unwrap();
                let num_rows =
                    state.image_paths.len().div_ceil(state.images_per_row);
                list.set_item_range(cx, 0, num_rows);
                while let Some(row_idx) = list.next_visible_item(cx) {
                    let row = list.item(cx, row_idx, live_id!(ImageRow));
                    row.draw_all(
                        cx,
                        &mut Scope::with_data_props(state, &row_idx),
                    );
                }
            }
        }
        DrawStep::done()
    }
}
```

In our new implementation, we first retrieve the state from the scope:
```
                let state = scope.data.get_mut::<State>().unwrap();
```

Next, we compute the number of rows based on the state:
```
                let num_rows =
                    state.image_paths.len().div_ceil(state.images_per_row);
                list.set_item_range(cx, 0, num_rows);
```

Then, we use `Scope::with_data_props` to create a new scope, containing a mutable reference to our `State`, as well as a reference to the index of the current row, and pass it to the current row:
```
                row.draw_all(cx, &mut Scope::with_data_props(state, &row_idx));
```

This lets each row know which images it is responsible for drawing.
### Using the State in `ImageRow`
Next, let's see how our state is used in `ImageRow`.

Replace the implementation of the `draw_walk` method for `ImageRow` with the one here below:
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
                let state = scope.data.get_mut::<State>().unwrap();
                let row_idx = scope.props.get::<usize>().unwrap();
                let first_image_idx = row_idx * state.images_per_row;
                let remaining_image_count =
                    state.image_paths.len() - first_image_idx;
                let item_count =
                    state.images_per_row.min(remaining_image_count);
                list.set_item_range(cx, 0, item_count);
                while let Some(item_idx) = list.next_visible_item(cx) {
                    if item_idx >= item_count {
                        continue;
                    }
                    let image_idx = row_idx * state.images_per_row + item_idx;
                    let image_path = &state.image_paths[image_idx];
                    let item = list.item(cx, item_idx, live_id!(ImageItem));
                    let image = item.image(id!(image));
                    image
                        .load_image_file_by_path_async(cx, &image_path)
                        .unwrap();
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
    
    ...
}
```

In our new implementation, we first retrieve the state and the index of the current row from the scope:
```
                let state = scope.data.get_mut::<State>().unwrap();
                let row_idx = scope.props.get::<usize>().unwrap();
```

Next, we compute the number of items that belong in this row:
```
                let first_image_idx = row_idx * state.images_per_row;
                let remaining_image_count =
                    state.image_paths.len() - first_image_idx;
                let item_count =
                    state.images_per_row.min(remaining_image_count);
                list.set_item_range(cx, 0, item_count);
```

**Note:** Normally, the number of items per row is determined by the value of the `images_per_row` field on the `State` struct, but for the last row it can be less than that if the number of remaining images is less than that.

Then, for each item, we first compute the index of the corresponding image.
```
                    let image_idx = first_image_idx + item_idx;
```

Next, we use this index to retrieve the path of the corresponding image:
```
                    let image_path = &state.image_paths[image_idx];
```

The following code:
```
                    let item = list.item(cx, item_idx, live_id!(ImageItem));
                    let image = item.image(id!(image));
                    image
                        .load_image_file_by_path_async(cx, &image_path)
                        .unwrap();
```
- Calls `list.item(...)` to get a reference to an `ImageItem`.
- Calls `item.image(...)` to get a reference to the `Image` inside it
- Calls `image.load_image_file_by_path_async(...)` to reload the `Image`.
- Calls `item.draw_all(...)` to redraw the `ImageItem` to the screen.

The net result of this is that the `Image` inside each `ImageItem` is reloaded with the correct image right before the `ImageItem` is drawn. Image loading happens asynchronously, and is handled behind the scenes.
## Checking our Progress so far
Let's check our progress so far.

Make sure you’re in your package directory, and run:
```
cargo run --release -- path/to/your/images
```

If everything is working correctly, the image grid should now display your images:
![[Dynamic Image Grid.png]]We now have a pretty decent implementation of an image grid. It can display actual images, and dynamically change both the number of rows and number of items per row based on the number of images.

We'll leave the image grid for what it is right now. In the next step, we're going to build a slideshow for our app.