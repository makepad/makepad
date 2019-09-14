# Makepad - Live coding 2D/VR IDE for Rust 
This repo will be restructured soon! To set the expectations: The current business model idea is to have all library crates be MIT open-source (editor, UI, vectors, renderstack, etc), but to have 'pro features' of the IDE like the VR compile connectivity, or visual design tools be paid as a subscription model. This means all software you develop with the makepad library stack will always have all dependencies be MIT, but we'll try to convince you that the pro features of the IDE are useful to pay for. This means that this repo will have an open part (what you find here today+lots more) and a closed part (currently starting the work).

Makepad: Code your design
Live demo: https://makepad.github.io/makepad/

Right now all makepad can do is 'dogfood' its own development in 2D, which means we use it ourselves every day to write its own code.

The vision of makepad is to build a livecoding / design hybrid IDE. The entire library stack you use to write programs is opinionated and integrated with IDE tools as much as possible. You can live modify values in code that update the running program (in 2D and VR), and it tries to recompile real code changes code as 'live' as it can. The language its written in, and targets is Rust and only Rust. The IDE will run both in 2D and in VR.  Makepad only depends on a few Rust only platform bindings and Serde, everything else compiles trivially on all supported platforms (gnu-windows as well). Makepad includes its own Rust based shader abstraction that can target GLSL, HLSL, MetalSL directly.

If you have missed 'learnable programming' please check this out: http://worrydream.com/LearnableProgramming/
Makepad aims to fulfill (some) of these ideas using a completely from-scratch renderstack built on the GPU and Rust. The first goal of makepad will be to be an IDE for VR that runs natively on your desktop OS, whilst generating/debugging native VR code that runs on standalone VR devices like the Oculus Quest. It will possibly also generate wasm+webXR but that depends on the stability of these API's. Sofar testing showed that its not ready.

We are not accepting pull requests or feature requests or documenting the code for now. However bugreports of obviously overlooked things are welcome. As the stack gets closer to beta we'd love to get more feedback. 

IMPORTANT INFO RUNNING THE LOCAL VERSION:
We are dogfooding the local version ourselves, but it needs a 'current project' which is hardcoded for now. Check out makepad repo 'again' under ./edit_repo in the root with:

./clone_edit_repo.sh<br/>
Start makepad:<br/>
cargo run -p makepad --release<br/>

Features:

Renderstack 2D - Currently working on new Font stack for VR/LowDPI fonts<br/>
UI Library - Mostly there, needs textinput control/dropwdown/menu<br/>
3D/VR rendering - Work just started<br/>
Code editor - Mostly there<br/>
Shader compiler - Mostly there<br/>
Buildsystem - Work just started<br/>
Platforms:<br/>
OSX + Metal - WORKING<br/>
Win32 + DirectX11 - WORKING<br/>
WASM + WebGL - WORKING<br/>
Linux + OpenGL - WORKING<br/>
Quest Android + OpenGL - TODO<br/>


Wasm:

Since webassembly can't really use a void main(){} because of the message loop,
makepad uses a macro, in main.rs you see: main_app!(App, "My App!");
This macro generates a main function for 'desktop' and a bunch of extern "C" exports for wasm that serve as the App event entrypoint. If you want to see how that works look in widget/render/src/cx.rs 

Application Structure:

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
Most layout engines work by first spawning a structure in memory, then running an algorithm to figure out where everything goes on the 'entire' structure, and then drawing it. The turtle approach allows you to layout 'as you draw'. This means that as you draw the UI the layout executes at the same time and is not a separate pass afterwards. Consider this puzzle: If you could only move things after you drew them (modify the gpu vertex data), what kind of layout engine could you devise if you want align bottom/right/center and content sizing/margins/padding to work. Thats what the Turtle stack does.
Makepad is almost entirely 'matrix' free, as in it just has a camera matrix.
All gpu data is generated absolutely positioned as draw runs. This is because otherwise you'd have to do tons of matrix muls/inversions in the eventflow. This is slow. 

Clipping is done using vertex-shader clipping. So there is no stencil-state. Makepad is really just a bunch of instanced-array drawcalls on indexed triangles. And one texture for the font. Thats it.

Font rendering in makepad is via an MSDF font format on master. We are in the final stages of integrating our own font stack (Rust ttf reader all the way to GPU trapezoid rendering and atlassing). This will significantly improve lowDPI/VR font clarity.
