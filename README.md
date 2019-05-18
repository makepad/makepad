# makepad
Livecoding Rust for 2D CAD 

Live demo: https://makepad.github.io/makepad/

This repo is for developing Makepad, a livecoding Rust IDE for 2D CAD design. The goal of the containing code is to be a Code Editor / UI kit for the Makepad application and will change without notice to suit that goal. The makepad application itself may be closed-sourced and sold, since we also have to feed our families.

The vision is to build a livecoding / design hybrid program, where procedural design and code are fused in one environment. If you have missed 'learnable programming' please check this out: http://worrydream.com/LearnableProgramming/
Makepad aims to fulfill (some) of these ideas using a completely from-scratch renderstack built on the GPU and Rust. It will be like an IDE meets a vector designtool, and had offspring. Direct manipulation of the vectors modifies the code, the code modifies the vectors. And the result can be lasercut, embroidered or drawn using a plotter. This means our first goal is 2D path CNC with booleans (hence the CAD), and not dropshadowy-gradients.

However before we can make this awesome application, we need to build a UI stack. Making a live-code editor with all sorts of visual manipulation components just doesn't work in HTML. We tried. It also doesn't work in JS+WebGL. We tried. But now with Rust we are getting right performance figures (up to 200x HTML). And even with Wasm, its plenty fast.
This new UI stack has a new way of building UI called 'dual immediate mode' and uses multi-platform shaders for styling that are compiled from Rust source via a proc macro.
Dual immediate mode has the code simplicity of an immediate mode API with the scalability/componentisation of a retained mode API.

The aim of this toolkit is to be our stepping stone into building a livecoding IDE and designtools that don't suck or fall to pieces along the way. We do not aim to maintain this library as everyones new web-UI lib, since we don't have the time and resources to do so. We are just a tiny company.

However since openness is fine and this may be useful to someone, the foundational technologies (UI/rendering/vector engine) will be MIT opensource, but provided AS IS.

We are not accepting pull requests or feature requests or documenting the code for now. However bugreports of obviously overlooked things are welcome.
You may use, reuse, fork, do whatever you feel like. But our focus is code/designtools and these libraries are just a tool for us to get there. As the technology looks like its working this time we will contemplate our open-source strategy and update this readme along the way.

IMPORTANT INFO RUNNING THE LOCAL VERSION:
The native version is being dogfooded on itself, so when you clone it run this first:
./run_first_init.sh
then run
./build_native.sh

Platforms:

WebGL compiles cleanly to wasm32-unknown-unknown without emscripten/wasmbindgen

Native metal backend on OSX without winit dep for UI

Desktop OGL on linux/windows, uses winit/glutin

Todo: DX11 / win32 native support

The project is split out over a few nested crates

src/main.rs - application 'main'

webgl/ - webGL build crate info (actual source is src/main.rs)

widget/src/*.rs - nested crate for the widgets

widgets/render/src/*.rs - nested crate for the render engine

The only 'web' JS code sits here, its a typed-array RPC driven simplification of the webGL API
widgets/render/src/cx_webgl.js

Prerequisites: Install cargo-watch, i use it for livecoding/building
cargo install cargo-watch

./serve_webgl.sh (uses node to start a tiny server on 127.0.0.1:2001)

./watch_webgl.sh - compiles the webGL thing

./watch_native.sh - compiles the native app on demand

You can choose the render backend by editing ./Cargo.toml
At the bottom is features=["mtl"], default set for OSX
set it to features=["ogl"] if you are on win/linux 

Since webassembly can't really use a void main(){} because of the message loop,
makepad uses a macro, in main.rs you see: main_app!(App, "My App!");
This macro generates a main function for 'desktop' and a bunch of extern "C" exports for wasm that serve as the App event entrypoint. If you want to see how that works look in widget/render/src/cx.rs 

Applications are nested structs. Widgets are simply structs you embed on the parent struct. Some widgets are 'generic' as in they have type args: Dock<Panel>.

Rust has this Trait called 'Default', its a way of using a splat for a struct initialisation. MyStruct{prop:10, ..Default::default()}. In makepad we have a Trait called Style. Style is exactly like default except it gets an extra argument called cx. This cx is needed because in Style things like shaders and fonts are gathered. We use implementations of Style per widget to set default values. 

What is Cx? Cx is a large struct (widget/render/src/cx.rs) that contains all of the render state. This means fonts, textures, shaders, rendertree. Its one of the 2 main values you will deal with. The other one is the 'self' of the struct your widget represents.

Every widget generaly has 2 functions. One is called handle_(widget), the other one is draw_(widget). These functions are NOT an interface, you can name them however you want since the parent calls these functions directly, and by name. But naming them handle_mywidget and draw_mywidget is the convention. That they are not interfaces is important because it means the system never re-enters the codeflow at a random point in your UI tree. Events /draw flow always starts at the absolute top of the application, and bubbles all the way through it at every single event / drawflow. This can be done intelligently (as in dont always flow everywhere), however its important to realise it always starts at the top. 

Some widgets have their 'draw_widget' function spread out over a begin/drawitem/end set of functions, this means you can use widgets as tiny render-libraries for a wrapper widget (see codeeditor.rs, which is wrapped by rusteditor.rs). The idea is that most widgets don't own their data, but the parent-widget uses a begin/draw/end structure to draw the data itself using the widgets' immediate mode render api. This solves a huge datamapping problem. Note however that with Filetree the easiest way was to specialise the widget entirely for the data. There is a cut-off point where generalising immediate mode APIs becomes a losing game. However if you ever saw a win32 filetree widget you would also say that writing it yourself actually is easier than using the widget APIs after some complexity point. 

Using a dual-api (handle and draw) comprises a new type of imgui. If you haven't heard of 'dear imgui' i suggest looking up some tutorials. Makepad is different in signficant ways though. In Imgui both the draw and eventflow are one single function. In makepad we splice out the draw and eventflow in 2 functions, draw and handle, with a struct as the backing connector between those 2. This solves major shortcomings in imguis in terms of performance, eventflow complexity and providing the missing lifecycle systems.

Whats fantastic about having an interface-free immediate mode event system, is that widget-events flow 'back up' to parent widgets as return values of their 'handle' functions. And because these aren't an interface, and Rust is statically typed, you can return any type you want as a return value.
Is your widget returning many events? Return a vec, is it returning a particular type? Return that. And because the parent usually knows its context, it can do something useful with it or bubble the event up further if need be. This model is proving to work exceptionally well, and obsoletes the need for closures or methods like 'onclick'. But Rust has closures you say. Yes, but JS like onclick and eventhandler methods really only work well with a GC. Rusts closures are 'functional' closures. Ie they work best having a lifetime the same as the functioncall they are embedded in. Don't do this and you won't be very happy with the borrowchecker.

Rendering in makepad is done using Shaders written in Rust. For an example look up src/widgets/render/src/quad.rs This is the most basic render primitive, 2 triangles in a rectangular shape with a fixed color. If you look at the button.rs widget you see you can override these shader functions for localised parameterisable styling.
Draw primitives such as Quad are included on the parent widgets just like other widgets are. They compose in exactly the same way.
If you want a 'quad' to be a background to something use the begin/end function pairs. It uses the turtle layout engine to do this.

Layout is handled using 'Turtles' in makepad. Its really turtles all the way down here. Turtles are a form of nested rectangular layout and locally a turtle 'walks' around the space calculating bounding boxes and alignment info as you draw things. As the turtle 'ends' it provides its bounding info to its parent turtle so it that can do things with just emitted GPU data like 'align bottom'. 
Most layout engines work by first spawning a structure in memory, then running an algorithm to figure out where everything goes on the 'entire' structure, and then drawing it. The turtle approach allows you to layout 'as you draw'. This means that as you draw the UI the layout executes at the same time and is not a separate pass afterwards. Consider this puzzle: If you could only move things after you drew them (modify the gpu vertex data), what kind of layout engine could you devise if you want align bottom/right/center and contensizing/margins/padding to work. Thats what the Turtle stack does.
Makepad is almost entirely 'matrix' free, as in it just has a camera matrix.
All gpu data is generated absolutely positioned as draw runs. This is because otherwise you'd have to do tons of matrix muls/inversions in the eventflow. This is slow. 

Clipping is done using vertex-shader clipping. So there is no stencil-state. Makepad is really just a bunch of instanced-array drawcalls on indexed triangles. And one texture for the font. Thats it.

Font rendering in makepad is via an MSDF font format. It doesn't do all sorts of internationalisation layout, its really very well suited for building code editors, IDE's, designtools. Not for replacing a browser. Maybe someday Mozilla will provide the missing pieces to do font rendering properly.
