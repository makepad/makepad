//! This is an example of a minimal Makepad application. It creates a single window, and renders a
//! rectangle to it.
//! ```
//! use makepad_render::*;
//! struct HelloWorldApp {
//!    window: Window,
//!    pass: Pass,
//!    color_texture: Texture,
//!    view: View,
//!}
//!
//!fn main(){
//!   main_app!(HelloWorldApp);
//!}
//!impl HelloWorldApp {
//!    pub fn shader() -> ShaderId {uid!()}
//!
//!    pub fn new(cx: &mut Cx) -> Self {
//!        Self::shader().set(cx, Quad::def_quad_shader());
//!        Self {
//!            window: Window::new(cx),
//!            pass: Pass::default(),
//!            color_texture: Texture::default(),
//!            view: View::new(cx),
//!        }
//!    }
//!    
//!    fn handle_app(&mut self, _cx: &mut Cx, _event: &mut Event) {}
//!    
//!    fn draw_app(&mut self, cx: &mut Cx) {
//!        self.window.begin_window(cx);
//!        self.pass.begin_pass(cx);
//!        self.pass.add_color_texture(cx, &mut self.color_texture, ClearColor::ClearWith(color256(32, 0, 0)));
//!        if self.view.begin_view(cx, Layout::default()).is_ok() {
//!            let mut quad = Quad::new(cx);
//!            quad.shader = App::shader().get(cx);
//!            quad.draw_quad_abs(cx, Rect{ x: 200.0, y: 150.0, w: 400.0, h: 300.0 });
//!            self.view.end_view(cx);
//!        }
//!        self.pass.end_pass(cx);
//!        self.window.end_window(cx);
//!    }
//!}
//!```
//!
//! # The `HelloWorldApp` struct
//!
//! At the top level, a Makepad application consists of a single `App` struct. `HelloWorldApp` contains all
//! the fields and methods that govern the application's behavior.
//!
//! ## Fields
//!
//! A minimal `App` contains fields with an instance of the following structs:
//!
//! * `Window`
//!   Represents a window into the application. We need at least one window if we want to display
//!   anything on the screen. Note that `Window` is part of Makepad's low-level API. As such, the
//!   corresponding window does not have any chrome. In a future tutorial, we will show how to
//!   create a window with chrome.
//!
//! * `Pass`
//!   Represents a render pass on the GPU. Think of a render pass as providing a render target for a
//!   sequence of rendering operations. This render target is usually a window, but it can be a
//!   texture as well. We need at least one render pass to render anything to the main window. We
//!   could also use multiple render passes to first render something to a target texture, and then
//!   use that texture as a source in subsequent render passes. For the time being, however, we will
//!   stick with a single render pass.
//!
//! * `View`.
//!   Represents a sequence of render commands. To understand why we need views, we first need to
//!   understand a bit more about the Makepad API. The Makepad API is *immediate mode*. That means
//!   each render command issued with this API behaves as if it is executed immediately. Immediate
//!   mode APIs are easy to use and reason about, Unfortunately, they do not map well to how GPUs
//!   actually work. Immediately executing each render command on the GPU as it is issued would be
//!   very inefficient. Therefore, what Makepad actually does behind the scenes is record each
//!   render command in a view as it is issued, and then execute all commands at once at a later
//!   time.
//!
//! ## Methods
//!
//! A minimal `App` contains the following methods:
//!
//! * `new`
//!   Creates an instance of `App`.
//!
//! * `handle_app`
//!   Handles an event for the application. We will have much more to say about event handling in
//!   future tutorials, but for the time being, we will simply ignore it.
//!
//! * `draw_app`
//!   Draws the application to the screen. We will discuss this method in more detail here below.
//!
//! # The `main_app` macro
//!
//! The `main_app` macro generates the code for initializing the application and running the main
//! event loop. `main_app` takes an `App` as argument, on which the methods we introduced above should all be
//! defined, and makes sure these methods are called at the appropriate time.
//!
//! # The `draw_app` function
//!
//! The `draw_app` method is where the application is drawn. This is where most of the magic
//! happens.
//!
//! The Makepad API is *stateful*. That means render commands are always issued and executed in a
//! particular render context. The `cx` argument is an instance of `Cx`, which represents the
//! current render context. Makepad has only one global render context, but it is passed as an
//! argument to `draw_app` for convenience. In general, any method that operates on the current
//! render context takes an instance of `Cx` as argument.
//!
//! The call to `self.window.begin_window(cx)` sets `self.window` as the current window. Any
//! subsequent calls will happen in the context of this window, until we reach the corresponding
//! call to `self.window.end_window(cx)`. Similarly, the call to `self.pass.begin_pass(cx)` sets
//! `self.pass` as the current render pass, until the corresponding call to `self.pass.end_pass(cx)`.
//!
//! The call to `self.view.begin_view(cx)` sets `self.view` as the current view. This call behaves a
//! bit differently than the others, because it returns a `Result`. The reason for this has to do
//! with how Makepad handles incremental redraw. Once the sequence of render commands that make up a
//! view has been recorded once, there is no reason to record the same sequence again, unless
//! something has changed. Consequently, the call to `begin_view` returns an error if it is called a
//! second time, unless the view has been explicitly marked as dirty.
//!
//! If the current view was set succesfully, we can start issueing render commands for it. These
//! render commands will be recorded on the view, and then executed at a later time in the context
//! of the current pass, which uses the current window as its default render target.
//!
//! For this tutorial, we will issue render commands to draw a single quad to the application's main
//! window. Before we can do so, however, we first need to create a shader that is capable of
//! rendering quads.
//!
//! ## Creating a shader
//!
//! Makepad actually features an advanced shader compiler that allows you to easily compose shaders
//! and integrate them with the rest of your code. A full explanation of the shader compiler is out
//! of scope for this tutorial, however, so for the time being, we will simply use one of Makepad's
//! built-in shaders. We will cover the shader compiler in-depth in a future tutorial.
//!
//! To create a shader that is capable of rendering quads, we simply call the static function
//! `Quad::def_quad_shader` in the constructor of `App`. Since creating shaders is expensive, we
//! need some place to store the resulting shader so we can reuse it in subsequent calls to
//! `draw_app`. However, unlike the other objects we've created so far, we will not store our shader
//! on `App`. Instead, we will store it on `Cx`.
//!
//! The reasons for this are hard to explain at this point, and have to do with how Makepad handles
//! styling and deferred shader compilation. A full explanation of these topics is out of scope for
//! this tutorial, so for now suffice it to say that shaders always have to be stored on `Cx`. We
//! will cover styling and deferred shader compilation in-depth in a future tutorial.
//!
//! Before we can store a shader on `Cx`, we first need to create a unique key to identify the
//! shader. As a convention, whenever we need such a key, we use a static function to create it.
//! This allows the name of the method to serve as the name of the key. In this case, we've created
//! a function `App::shader`. This function calls the macro  `uid`, which returns a unique key based
//! on its position in the source file. It then wraps this key in a `ShaderId` newtype, which
//! indicates that this key represents a shader, before returning it.
//!
//! There are several different types of object that we can store in `Cx`. The purpose of `ShaderId`
//! is to prevent us from accidentally conflating one type of object with another. To store a shader
//! on `Cx`, we call the method `set` on `ShaderId`. This method takes a shader object as argument,
//! so we can't accidentally store an object of a different type. To retrieve the shader from `Cx`
//! at a later time, we call the method `get` on `ShaderId`. This method always returns a shader
//! object, so we can't accidentally retrieve an object of a different type.
//!
//! ## Generating vertex data
//!
//! Now that we've created a shader that is capable of rendering quads, the final step is to provide
//! this shader with the vertex data it needs to render a single quad. To accomplish this, we create
//! an instance of `Quad` in `draw_app`. This is a simple helper struct used for recording vertex
//! data. Once we have an instance of `Quad`, we retrieve our shader object from `Cx`, and set it as
//! the current shader for this instance. Finally, we call the method `draw_quad_abs` on our
//! instance of `Quad`, which records the vertex data for rendering a single quad using the current
//! shader.

use makepad_render::*;

struct HelloWorldApp {
    window: Window,
    pass: Pass,
    view: View,
}

fn main(){
    main_app!(HelloWorldApp);
}

impl HelloWorldApp {
    pub fn shader() -> ShaderId {uid!()}

    pub fn new(cx: &mut Cx) -> Self {
        Self::shader().set(cx, Quad::def_quad_shader());
        Self {
            window: Window::new(cx),
            pass: Pass::default(),
            view: View::new(cx),
        }
    }
    
    fn handle_app(&mut self, _cx: &mut Cx, _event: &mut Event) {}
    
    fn draw_app(&mut self, cx: &mut Cx) {
        self.window.begin_window(cx);
        self.pass.begin_pass(cx);
        self.pass.set_window_clear_color(cx, color("grey"));
        if self.view.begin_view(cx, Layout::default()).is_ok() {
            let mut quad = Quad::new(cx);
            quad.shader = Self::shader().get(cx);
            quad.draw_quad_abs(cx, Rect{ x: 200.0, y: 150.0, w: 400.0, h: 300.0 });
            self.view.end_view(cx);
        }
        self.pass.end_pass(cx);
        self.window.end_window(cx);
    }
}
