# Makepad

This is Makepad, a new way to do UIs in Rust for native and web. 
Makepad consists of the following parts:
- Makepad Framework
- Makepad Examples
- Makepad Studio
- Makepad Designer

What these are is explained further down.

We also maintain online versions of the builds below:\
Fractal zoomer:\
[http://makepad.nl/makepad/examples/fractal_zoom/src/index.html](http://makepad.nl/makepad/examples/fractal_zoom/src/index.html)
Fun audio:\
[http://makepad.nl/makepad/examples/fun_audio/src/index.html](http://makepad.nl/makepad/examples/fun_audio/src/index.html)
Makepad Studio:\
[http://makepad.nl/makepad/studio/src/index.html](http://makepad.nl/makepad/studio/src/index.html)

### Prerequisites

Note: Our Linux and Windows platform layers are currently WIP and don't run. Build on MacOS for now.

In order to compile makepad applications you need to first install Rust.\
[https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

After installing rust, install the nightly toolchain\
```rustup toolchain install nightly```

Then to build the webassembly versions you need to install the wasm32 toolchain\
```rustup target add wasm32-unknown-unknown --toolchain nightly```

## Makepad Framework 
Open source UI framework that you can use to build UI's for web and native applications. 

Makepad framework applications are built straight onto the platform layer, and are small and lightweight.
We have platform layers for Web, MacOS, Linux and Windows. All rendering is GPU based and we use a shader DSL for all visual styling

## Makepad Examples 
Open source example applications to learn how to build makepad framework applications

### The fractal zoomer example:
To run natively use the following command (MacOS only for now):\
```cargo +nightly run -p fractal_zoom --release```

To run the webassembly version build with this command:\
```./tools/build_wasm_simd.sh fractal_zoom```

Start the webserver with:\
```cargo +nightly run -p webserver --release```

Then you can open this url in Chrome:\
[http://127.0.0.1:8080/makepad/examples/fractal_zoom/src/index.html](http://127.0.0.1:8080/makepad/examples/fractal_zoom/src/index.html)

### The fun audio example:
To run natively use the following command (MacOS only for now):\
```cargo +nightly run -p fun_audio --release```

To run the webassembly version build with this command:\
```./tools/build_wasm_simd.sh fun_audio```

Start the webserver with:\
```cargo +nightly run -p webserver --release```

Then you can open this url in Chrome:\
[http://127.0.0.1:8080/makepad/examples/fun_audio/src/index.html](http://127.0.0.1:8080/makepad/examples/fun_audio/src/index.html)

## Makepad Studio 

### What Makepad Studio Is

Makepad Studio is a prototype of a code editor written in Makepad. For now, it is primarily intended to show off how one could write their own code editor in Makepad. Our eventual goal is to evolve this into a feature complete, fully extendable Rust IDE.

At the moment of this writing, the following features are supported by Makepad Studio:

-   File tree
-   Basic edit operations
-   Undo/redo
-   Basic syntax highlighting (Rust only)
-   Basic collaborative editing

### What Makepad Studio Is Not

Makepad Studio is not intended to compete with existing IDEs, such as Visual Studio Code. There won't be an extension store. It's primary purpose is to serve as the open source foundation for our own commercial offering, as well as offer an extendible framework for others to build their own solutions with.

At the moment of this writing, the following features are not yet supported by Makepad Studio:

-   Unicode support
-   Search/replace
-   Regular expressions
-   Internationalization
-   Accessibility
-   Extensibility
    
### Build Instructions

To run natively use the following command (MacOS only for now):\
```cargo +nightly run -p makepad_studio --release```

To run the webassembly version build with this command:\
```./tools/build_wasm_normal.sh makepad_studio```

Start the webserver with:\
```cargo +nightly run -p webserver --release```

Then you can open this url in Chrome:\
[http://127.0.0.1:8080/makepad/studio/src/index.html](http://127.0.0.1:8080/makepad/makepad_studio/src/index.html)

## Makepad Designer 

Makepad Designer will be our commercially licensed designtool built as extension of Makepad Studio.
The designer will have a visual UI designer to allow creating makepad applications visually and unify the workflow of Designers and programmers in a new way.

## Contact

If you have any problems/questions, or want to reach out for some other reason, you can find our discord channel at:\
[https://discord.com/invite/urEMqtMcSd](https://discord.com/invite/urEMqtMcSd)

Keep in mind that we are a small team, so we might not always be able to respond immediately.

