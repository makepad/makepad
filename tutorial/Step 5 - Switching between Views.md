In the previous steps, we created both an image grid and a slideshow for our app. In this step, we'll make it possible to switch between the image grid and the slideshow.

Our app will start out displaying the image grid. Above the image grid we'll add a menu bar containing a single button. Clicking this button will switch to the slideshow. Pressing the escape key while the slideshow is displayed will switch back to the image grid.

At the end of this step, you'll be able to switch between the image grid and the slideshow.
## What you will learn
- How to use a `PageFlip` to switch between multiple views.
## Updating the DSL Code
As always, we'll start by updating the DSL code with the definitions we need.
### Defining a `MenuBar`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    MenuBar = <View> {
        width: Fill,
        height: Fit,

        <Filler> {}
        slideshow_button = <Button> {
            text: "Slideshow"
        }
    }
```

This code defines a `MenuBar`. A `MenuBar` is a container that sits above an ImageGrid, and contains the `Button` we'll use to switch to the slideshow.

This `TopMenu` has the following properties:
- `width: Fill` ensures the menu spans the width of the window.
- `height: Fit` ensures the menu takes up as much vertical space as needed.

Each `TopMenu` contains both a `Filler` and a `Button`:
```
        <Filler> {}
        slideshow_button = <Button> {
            text: "Slideshow"
        }
```

The `Filler` simply pushes the `Button` to the right. We've assigned the name `slideshow_button` to our button so we can refer to it later in our event handling code.
### Defining an `ImageBrowser`
Add the following code to the call to the `live_design` macro in `app.rs`:
```
    ImageBrowser = <View> {
        flow: Down,

        <MenuBar> {}
        <ImageGrid> {}
    }
```

This code defines an `ImageBrowser`. An `ImageBrowser` combines the `MenuBar` we created earlier with our `ImageGrid`.

This `ImageBrowser` has the following properties:
- `flow: Down` ensures the browser's children are laid out from top to bottom.
### Updating `App`
Replace the definition of `App` in the call to the `live_design` macro in `app.rs` with the one here below:
```
    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = {
                    page_flip = <PageFlip> {
                        active_page: image_browser,

                        image_browser = <ImageBrowser> {}
                        slideshow = <Slideshow> {}
                    }
                }
            }
        }
    }
```

Previously, we commented out our ImageGrid, and replaced it with our Slideshow. Now, we've replaced `ImageGrid` with `ImageBrowser`, and grouped it together with Slideshow into a `PageFlip`.

A `PageFlip` is a container for multiple widgets, only one of which is displayed at a time. Each widget inside a PageFlip is known as a **page**. The page that is currently being displayed is known as the **active page**. Pages are identified by name, which is used to set the active page.
## Updating the Rust Code
Next, let's update the Rust code with the definitions we need.

Replace the implementation of the `MatchEvent` trait for the `App` struct with the one here below:
```
impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(id!(slideshow_button)).clicked(&actions) {
            self.ui
                .page_flip(id!(page_flip))
                .set_active_page(cx, live_id!(slideshow));
            self.ui.view(id!(slideshow.overlay)).set_key_focus(cx);
        }
        if self.ui.button(id!(left)).clicked(&actions) {
            self.navigate_left(cx);
        }
        if self.ui.button(id!(right)).clicked(&actions) {
            self.navigate_right(cx);
        }
        if let Some(event) =
            self.ui.view(id!(slideshow.overlay)).key_down(&actions)
        {
            match event.key_code {
                KeyCode::ArrowLeft => self.navigate_left(cx),
                KeyCode::ArrowRight => self.navigate_right(cx),
                KeyCode::Escape => self
                    .ui
                    .page_flip(id!(page_flip))
                    .set_active_page(cx, live_id!(image_browser)),
                _ => {}
            }
        }
    }
}
```

This is pretty much the same code as before, except that we've added some additional event handling code. We'll go over the newly added code here below:
### Handling Button Clicks
The following code:
```
        if self.ui.button(id!(slideshow_button)).clicked(&actions) {
            self.ui
                .page_flip(id!(page_flip))
                .set_active_page(cx, live_id!(slideshow));
            self.ui.view(id!(slideshow.overlay)).set_key_focus(cx);
        }
```

checks whether the slideshow button was clicked. If so, it changes the active page of the page flip to the slideshow, and ensures the slideshow overlay has key focus. 
### Handling Key Presses
The following code:
```
                KeyCode::Escape => self
                    .ui
                    .page_flip(id!(page_flip))
                    .set_active_page(cx, live_id!(image_browser)),
```

checks whether the escape key was pressed while the slideshow overlay has key focus. If so, it changes the active page of the page flip back to the image browser.
## Checking our Progress so far
Let's check our progress so far.

Make sure you’re in your package directory, and run:
```
cargo run --release -- path/to/your/images
```

If everything is working correctly, a menu bar with a single slideshow button should now appear above the image grid:
![[Dynamic Image Grid.png]]
Clicking the button should cause the slideshow to appear:
![[Slideshow 1.png]]
Pressing the escape key should cause the image grid to appear again:
![[Dynamic Image Grid.png]]
We now made it possible to switch between the image grid and the slideshow. In the next step, we'll add a way to filter images based on a query string.