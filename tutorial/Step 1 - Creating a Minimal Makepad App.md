In this step, we're going to create a minimal Makepad app, consisting of a window with an empty view.
## What you will learn
In this step, you will learn:
- How to create a new Cargo package.
- How to add Makepad as a dependency.
- How to create a window with an empty view.
- How to use Makepad DSL code to define the layout and styling of our app.
## Creating a new Cargo Package
First, we're going to create a new Cargo package.

In Cargo, a **package** is a bundle of one or more crates. A **crate** is either a binary or a library crate. A **binary crate** is a program that compiles to an executable that we can run. A **library crate**, in contrast, does not compile to an executable, but defines functionality intended to be shared between multiple projects.

Since we're creating an app, we need a Cargo package with a single binary crate. To create it, run:
```
cargo new image-viewer
```

This will create a new directory named `image_viewer`. To navigate to it, run:
```
cd image_viewer
```

Now that we are in the `image_viewer` directory, we can compile and run the binary crate in our package. To do this, run:
```
cargo run --release
```

If everything is working correctly, the following text should appear on your terminal:
```
Finished `release` profile [optimized] target(s) in 0.02s
Running `target/release/image_viewer`
Hello, world!
```

**Note:**
By default, binary crates in Rust run in **debug mode**. In debug mode, the compiler does not optimise our code, and adds full debugging information. This makes our crate easier to debug, but also makes it slower. We prefer to run our crate in **release mode**. In release mode, the compiler heavily optimises our code, and strips out most debugging information. This better show off the speed of Makepad apps, but it also makes them harder to debug. If you ever need more debug information, you can always remove the `--release` flag.
## Adding Makepad as a Dependency
Now that we've created a new empty Cargo package, we're going to add Makepad as a dependency to it.

Rust crates are published on **crates.io**, the Rust community's central package registry for discovering and downloading crates. Whenever we add a crate as a dependency, Cargo uses crates.io by default to find the requested crate.

The top-level crate for Makepad is called `makepad_widgets`. It contains everything we need to build an app. To add it as a dependency, run:
```
cargo add makepad_widgets
```
## Adding the Code
Now that we've added Makepad as a dependency to our package, we're going to add the code we need to create our minimal Makepad app.

We'll look at what this code does in more detail shortly — for now, just copy the following files to your `src` directory:

`app.rs`:
```
use makepad_widgets::*;

live_design! {
    use link::widgets::*;

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {}
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
    }
}
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

app_main!(App); 
```

`lib.rs`:
```
pub mod app;
```

`main.rs`:
```
fn main() {
    image_viewer::app::app_main()
}
```

Once you've copied over the files, let's test our new app:
```
cargo run --release
```

If everything is working correctly, an empty window should appear on your screen:
![[Empty Window.png]]
## Going over the Code
Now that we've added the code we need, let's look at what it does in more detail:
## Calling the `live_design` macro
At the start of `app.rs`, there is a call to the `live_design` macro:
```
live_design! {
    use link::widgets::*;

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {}
            }
        }
    }
}
```

This macro is used to define Makepad **DSL code**, which we use to define the layout and styling of our app, similar to CSS.

Let's break it down:
- `use link::widgets::*;` imports Makepad's built-in widgets, such as `Root`, `Window`, and `View`.
- `App = {{App}} { ... }` defines an `App`. The `{{App}}` syntax tells Makepad that this definition corresponds to an `App` struct in the Rust code.
- Within `App`, we define the `ui` field as the root of our widget tree:
	- `<Root>` represents our top-level widget container.
	- `<Window>` represents a window on the screen.
	- `body = <View> {}` adds an empty view named `body` to our window.

Now that we've seen what DSL code looks like, let's take a look at how it is used:
#### The Live Design System
When we run our app, Makepad uses something called the **live design system** to bridge our DSL code with our Rust code.

The idea is this:
- We define the layout and styling of our app in DSL code.
- The live design system matches DSL definitions to their corresponding Rust struct, and initialises their fields with the values from the DSL.
- If the DSL code is later changed — for example, in Makepad Studio — the corresponding Rust structs are automatically updated at runtime, without restarting our app.
 
 The live design system is what enables Makepad’s powerful runtime styling capabilities.  You can use Makepad Studio to tweak the layout, colors, and other styling properties of the UI and instantly see the results in your running app — no recompilation required!
### Defining an `App` struct
The following code defines an `App` struct:
```
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}
```

This struct serves as the core of our app. For now, it just contains the root of our widget tree. Later on, we will also add any state we need for our app.

Note that we are deriving two traits for the `App` struct, `Live` and `LiveHook`. Let's take a closer look at those:
#### Deriving the `Live` trait
The `Live` trait enables a struct to interact with the live design system.

To derive the `Live` trait for a struct, each field on the struct needs to be marked with one of the following attributes:
- The `#[live]` attribute marks the field as part of the live design system.
- The `#[rust]` attribute marks it as an ordinary Rust field.

When the live design system encounters a field marked with the `#[live]` attribute, it looks for a matching definition in the DSL code. If it finds one, it will use that definition to instantiate the field. Otherwise, it instantiates it with a default value.

In contrast, when the live design systems encounters a field marked with the `#[rust]` attribute, it uses the `Default::default` constructor for values of that type to instantiate the field.

In our case, we mark the `ui` field of the `App` struct with the `#[live]` attribute. Since we have a corresponding definition for this field in the DSL, the live design system automatically populates it with the `Root` widget, as defined in the DSL. From there, the rest of the widget tree — `Window`, `View`, etc. — is built out based on the definitions in the DSL.

#### Deriving the `LiveHook` trait
The `LiveHook` trait provides several overridable methods that will be called at various points during our app's lifetime. We don't need any of these methods right now, so we could just define an empty implementation of the `LiveHook` for the `App` struct:
```
impl LiveHook for App {}
```

Alternatively, we can derive the `LiveHook` trait for the `App` struct, as we have done here. This will generate the same implementation as above, which saves us some typing.
### Implementing the `AppMain` trait
The following code implements the `AppMain` trait for the `App` struct:
```
impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
```
 
This trait is used to hook the `App` struct into the main event loop. In our implementation, we simply forward all events to the root of our widget tree, so it can handle them appropriately.
 
A **scope** in Makepad is a container that is used to pass both app-wide data and widget-specific props along with each event. Since we don't have any state yet, for now we just call `Scope::empty()` to create an empty scope for each event.

### Implementing the `LiveRegister` trait
The following code implements the `LiveRegister` trait for `App`:
```
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}
```

This trait is used to register DSL code. Registering DSL code is what makes it available to the rest of our app.

Earlier, we said that the `live_design` macro is used to define DSL code. Under the hood, it generates a `live_design` function that, when called, registers the DSL code with which the macro was called. The `LiveRegister` trait is where we call these functions.

In our implementation, we call the `makepad_widgets::live_design` function to register the DSL code in the `makepad_widgets` crate. Without this call, we would not be able to use any of the built-in widgets like `Root`, `Window`, and `View`  that we saw earlier.

**Note:**
The `live_design` function generated by the `live_design` macro at the top of `app.rs` is special. We don't need to call it here, because it is automatically called by the `app_main` function. This function is generated by the `app_main` macro, which we'll explain next.
### Calling the `app_main` macro
At the end of `app.rs`, there is a call to the `app_main` macro:
```
app_main!(App);
```

This macro generates an `app_main` function that contains all the code necessary to start our app.

Here's what the `app_main` function does:
- It uses the `LiveRegister` trait to register additional DSL code.
- It registers the DSL code at the top of the file.
- It uses the live design system to create an instance of the `App` struct.
- It uses the `AppMain` trait to hook this instance into the main event loop.
