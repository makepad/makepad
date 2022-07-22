# Makepad

This is Makepad, a new way to do UIs in Rust. At the moment, Makepad is primarily a UI framework, called Makepad Framework. In the future, Makepad will also be a UI designer application, called Makepad Designer, which we hope to build on top of Makepad Framework.

Applications written in Makepad Framework:
-   Are written in Rust
-   Can run natively and on the web
-   Are rendered entirely on the GPU
-   Are both small and fast

To give you an idea of the kind of applications you can build with Makepad, this repository contains both Makepad Studio, which is a prototype of a code editor that will eventually serve as the open source foundation for Makepad Designer, and several smaller examples.

## Makepad Studio

### What Makepad Studio Is

Makepad Studio is a prototype of a code editor written in Makepad. For now, it is primarily intended to show off how one could write their own code editor in Makepad. Our eventual goal is to evolve this into a feature complete, fully extendable IDE.

Our intention is to develop Makepad Designer as a commercially licensed extension on top of Makepad Studio. Makepad Studio itself will always remain free and open source.  

At the moment of this writing, the following features are supported by Makepad Studio:

-   File tree
-   Basic edit operations
-   Undo/redo
-   Basic syntax highlighting (Rust only)
    

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

#### Native

To run Makepad Studio natively, use the following command:\
```cargo run -p makepad_studio```

At the moment, Makepad Studio only runs natively on MacOS, but support for Windows/Linux is coming soon.

#### Web

To run Makepad studio on the web, use the following commands:\
```tools/build_wasm_normal.sh makepad_studio```\
```cargo run -p webserver --release```

Once the web server is running, open the following URL in a browser:\
[http://127.0.0.1:8080/makepad/studio/src/index.html](http://127.0.0.1:8080/makepad/studio/src/index.html)

## Examples

<TODO>

## Contact

If you have any problems/questions, or want to reach out for some other reason, you can find our discord channel at:\
[https://discord.com/invite/urEMqtMcSd](https://discord.com/invite/urEMqtMcSd)

Keep in mind that we are a small team, so we might not always be able to respond immediately.

