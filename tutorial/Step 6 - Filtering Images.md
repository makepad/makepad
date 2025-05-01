In the previous steps, we created both an image grid and a slideshow for our app, and made it possible to switch between the two. In this step, we'll add a way to filter images based on a query string.

Our plan of attack will be this:
- First, we'll create a search box, and add it to the menu bar we created in the previous step.
- Next, we'll update the state to keep track of which subset of the images should be displayed.
- Finally, we'll update our drawing code to use the new state.

At the end of this step, you'll be able to filter images by typing in a search box.
## What you will learn
In this chapter, you will learn:
- How to use an `Icon` to display icons.
- How to use a `TextInput` for inputting text.
## Adding Resources
For the search box, we'll need a looking glass icon, so let's start by adding that as a resource to our app.

Navigate to the `resources` directory, and then download the following files to it:
[[looking_glass.svg]]

We'll be using this file as our looking glass icon.
## Updating the DSL Code
As always, we'll start by updating the DSL code with the definitions we need.
### Defining Variables
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    LOOKING_GLASS = dep("crate://self/resources/looking_glass.svg");
```

This code defines a variable named `LOOKING_GLASS` to refer to the looking glass icon that we added to the `resource` directory earlier.
### Adding a `SearchBox`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    SearchBox = <View> {
        width: Fit,
        height: Fit,
        align: { y: 0.5 }
        margin: { left: 60 }

        <Icon> {
            icon_walk: { width: 12.0 }
            draw_icon: {
                color: #8,
                svg_file: (LOOKING_GLASS)
            }
        }

        query = <TextInput> {
            empty_text: "Search",
            draw_text: {
                text_style: { font_size: 10 },
                color: #8
            }
        }
    }
```

This code defines a `SearchBox`. A `SearchBox` is a simple container that combines 
an `Icon` with a `TextInput`. An `Icon` is a simple widget that displays an icon, whereas a ` TextInput` is used for inputting text.

This `SearchBox` has the following properties:
- `width: Fit` and `height: Fit` ensure that the search box takes up as much space as needed.
- `align { y: 0.5 }` ensures that the search box is vertically centered.
- `margin { left: 60 }` ensures the search box has a slight margin on the left (so it does not overlap with the window buttons).

The `Icon` in the search box has the following properties:
- `icon_walk { ... }` controls how the icon is laid out.
	- `width: 12` makes the icon 12 pixels wide.
- `draw_icon { ... }` controls how the icon is drawn.
	- `color: #8` ensures the icon is red.
	- `svg_file: (LOOKING_GLASS)` sets our looking glass icon as the SVG file for this icon.

The `TextInput` in the search box has the following properties:
- `empty_text: "Search"` ensures the string "Search" is displayed when the input is empty.
- `text_style: { ... }` controls how the text is styled.
	- `font_size: 10` ensures the text has a size of 10 points.
- `color: #8` ensures the text is red.

 We've assigned the name `query` to our text input so we can refer to it later in our state updating code.

### Updating `MenuBar`
Replace the definition of `MenuBar` in the call to the `live_design` macro in `app.rs` with the one here below:
```
    MenuBar = <View> {
        width: Fill,
        height: Fit,

        <SearchBox> {}
        <Filler> {}
        slideshow_button = <Button> {
            text: "Slideshow"
        }
    }
```

This adds our `SearchBox` to the `MenuBar`. We've added it before the `Filler` to ensure that it is laid out on the left.
## Extending the State
Now that we've updated the DSL code with the definitions we need, it's time to extend the state for our app with some additional fields and methods we need for filtering images.

Specifically, we’ll add a field to track which subset of images should be displayed, and a few helper methods for updating that state at runtime. We'll also need to update some of our existing helper methods.
### Updating the State Struct
Replace the definition of the `State` struct and its corresponding implementation of the `Default` trait with the one here below:
```
#[derive(Debug)]
pub struct State {
    image_paths: Vec<PathBuf>,
    filtered_image_idxs: Vec<usize>,
    images_per_row: usize,
    current_image_idx: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            image_paths: Vec::new(),
            filtered_image_idxs: Vec::new(),
            images_per_row: 4,
            current_image_idx: None,
        }
    }
}
```

As you can see, we've added one additional field to the `State` struct:
- `filtered_image_idxs` contains a list of indices into `image_paths`.

The idea is that we're going to add a level of indirection. Instead of iterating over/indexing into `image_paths` directly, as we did previously, we're now going to iterate over/index into `filtered_image_idxs`, and then use *those* indices to index into `image_paths`. This allows us to define a filtered subset of images to display, while leaving the original images unchanged.

Moreover, `current_image_idx` now contains an index into `filtered_image_idxs`, allowing the slideshow to navigate over the filtered images instead of the original.

**Note:**
To minimise confusion, we'll adopt the following naming conventions:
- Previously, we used the name `image_idx` to refer to indices that are used to index into `image_paths`.
- Now, we'll use the name `image_idx` to refer to indices that are used to index into `filtered_image_idxs`.
- We'll use the name `filtered_image_idx` to refer to the indices in `filtered_image_idx`. Only those indices can be used to index into `image_paths`.

### Adding Helper Methods
To facilitate image filtering, we're going to add a new helper method to the `App` struct.
#### Adding the `filter_image_paths` method
The `filter_image_paths` method is used to filter the list of image paths based on a query string:
```
impl App {
	pub fn filter_image_paths(&mut self, cx: &mut Cx, query: &str) {
        self.state.filtered_image_idxs.clear();
        for (image_idx, image_path) in self.state.image_paths.iter().enumerate()
        {
            if image_path.to_str().unwrap().contains(&query) {
                self.state.filtered_image_idxs.push(image_idx);
            }
        }
        if self.state.filtered_image_idxs.is_empty() {
            self.set_current_image(cx, None);
        } else {
            self.set_current_image(cx, Some(0));
        }
    }
}
```

Here's what the `filter_image_paths` method does:
- First, it removes any existing indices from `filtered_image_idxs`.
- Next, it iterates over all paths in `image_paths`.
- For each path:
	- It checks whether the path matches the query string.
	- If it does not, the path is skipped.
	- Otherwise, the path's index is added to `filtered_image_idxs`.
### Updating the `set_current_image` method
Replace the definition of the `set_current_image` method on `App` with the one here below:
```
    pub fn set_current_image(&mut self, cx: &mut Cx, image_idx: Option<usize>) {
        self.state.current_image_idx = image_idx;
        let image = self.ui.image(id!(slideshow.image));
        if let Some(image_idx) = self.state.current_image_idx {
            let filtered_image_idx = self.state.filtered_image_idxs[image_idx];
            let image_path = &self.state.image_paths[filtered_image_idx];
            image
                .load_image_file_by_path_async(cx, &image_path)
                .unwrap();
        } else {
            image
                .load_image_dep_by_path(cx, self.placeholder.as_str())
                .unwrap();
        }
        self.ui.view(id!(slideshow)).redraw(cx);
    }
```

The only thing that changed here is the following code:
```
            let filtered_image_idx = self.state.filtered_image_idxs[image_idx];
            let image_path = &self.state.image_paths[filtered_image_idx];
            image
                .load_image_file_by_path_async(cx, &image_path)
                .unwrap();
```

Here's what this code does:
- It uses the current image index to obtain the corresponding filtered image index.
- It uses this filtered image index to obtain the corresponding path.
- It reloads the `Image` using this path.

This does exactly what we said we'd do earlier: instead of indexing into `image_paths` directly, as we did previously, we now index into `filtered_image_idxs`, and then use that index to index into `image_paths`. The net result of this change is that the slideshow only displays filtered images.
### Updating the `navigate_left/navigate_right` methods
Replace the definition of the `navigate_left`/`navigate_right` methods on `App` with the ones here below:
```
    pub fn navigate_left(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx > 0 {
                self.set_current_image(cx, Some(image_idx - 1));
            }
        }
    }

    pub fn navigate_right(&mut self, cx: &mut Cx) {
        if let Some(image_idx) = self.state.current_image_idx {
            if image_idx + 1 < self.state.filtered_image_idxs.len() {
                self.set_current_image(cx, Some(image_idx + 1));
            }
        }
    }
```

These methods are essentially unchanged, except that they now operate on the list of filtered images:
- `navigate_left` first decrements `current_image_idx` by 1, unless we're already at the first *filtered image*.
- `navigate_right` first increments `current_image_idx` by 1, unless we're already at the last *filtered image*.
- Both methods then call `set_current_image` to apply the change, and schedule the slideshow to be redrawn.

The net result of this change is that the slideshow only navigates to filtered images.
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
        let query = self.ui.text_input(id!(query)).text();
        self.filter_image_paths(cx, &query);
    }
}
```

Previously, the following 4 lines appeared at the end:
```
        if self.state.image_paths.is_empty() {
            self.set_current_image(cx, None);
        } else {
            self.set_current_image(cx, Some(0));
        }
```

These have now been replaced with:
```
        let query = self.ui.text_input(id!(query)).text();
        self.filter_image_paths(cx, &query);
```

Here's what this code does:
- It obtains the current query string from the search box.
- If calls `filter_image_paths` the list of image paths with the current query string.

The idea is that every time the list of images is updated, we need to redo the image filtering. Since the current image is based on the filtered image list, setting the current image is now done in the `filter_image_paths` method, so we no longer need to do it here.

## Updating the Drawing Code
Finally, let's update our drawing code to use the new state.
### Updating  the `ImageRow` struct
Replace the definition of the `draw_walk` method in the implementation of the `Widget` trait for the `ImageRow` struct with the one here below:
```
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
                    state.filtered_image_idxs.len() - first_image_idx;
                let item_count =
                    state.images_per_row.min(remaining_image_count);
                list.set_item_range(cx, 0, item_count);
                while let Some(item_idx) = list.next_visible_item(cx) {
                    if item_idx >= item_count {
                        continue;
                    }
                    let image_idx = first_image_idx + item_idx;
                    let filtered_image_idx =
                        state.filtered_image_idxs[image_idx];
                    let image_path = &state.image_paths[filtered_image_idx];
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
```

That's quite a lot of code, but there's really only a few things that have changed here.

First, there is this line:
```
                let remaining_image_count =
                    state.filtered_image_idxs.len() - first_image_idx;
```

All this does is change the computation of the number of remaining images to take into account that we are now only drawing filtered images, instead of every image (by replacing `state.image_paths.len()` with `state.filtered_image_idxs.len()`).

Next, there are the following lines:
```
                    let filtered_image_idx =
                        state.filtered_image_idxs[image_idx];
                    let image_path = &state.image_paths[filtered_image_idx];
```

This does the same thing we did in the `set_current_image` method earlier: instead of indexing into `image_paths` directly, as we did previously, we now index into `filtered_image_idxs`, and then use that index to index into `image_paths`. The net result of this change is that the image grid only displays filtered images.
### Updating the `ImageGrid` struct
Replace the definition of the `draw_walk` method in the implementation of the `Widget` trait for the `ImageGrid` struct with the one here below:
```
    fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        walk: Walk,
    ) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                let state = scope.data.get_mut::<State>().unwrap();
                let num_rows = state
                    .filtered_image_idxs
                    .len()
                    .div_ceil(state.images_per_row);
                while let Some(row_idx) = list.next_visible_item(cx) {
                    if row_idx >= num_rows {
                        continue;
                    }
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
```

Once again, that's quite a lot of code, but the only real change here is the following line:
```
                let num_rows = state
                    .filtered_image_idxs
                    .len()
                    .div_ceil(state.images_per_row);
```

All this does is change the computation of the number of rows to take into account that we are now only drawing filtered images, instead of every image (by replacing `state.image_paths.len()` with `state.filtered_image_idxs.len()`).
## Checking our Progress so far
Let's check our progress so far.

Make sure you’re in your package directory, and run:
```
cargo run --release -- path/to/your/images
```

If everything is working correctly, you should now be able to filter images by typing in the search box at the top:
![[Image Filtering.png]]
We now have a way to filter images based on a query string.