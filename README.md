# Introducing Makepad

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications.

As we're working towards our first public alpha version, you'll be able to see our final steps towards it here. The alpha version of Makepad will show off the development platform, but does not include the visual design tools or library ecosystem yet.

Makepad currently boasts the following features:
- Compiles natively to Linux, MacOS, and Windows.
- Compiles to WebAssembly for demo purposes (see caveats below).
- Has a built-in HTTP server with live reload support for WebAssembly development.
- Has a code editor with live code folding (press alt).
- Has a log viewer with a virtual viewport, that can handle printlns in an infinite loop.
- Has a dock panel system / file tree.
- Has Rust compiler integration, with errors/warning in the IDE.

Note that although Makepad compiles to WebAssembly, and therefore runs on the web, the web build of Makepad does not feature any compiler integration. If you want to be able to compile code, you have to install Makepad locally.

To install Makepad locally, run the following commands:
```
git clone https://github.com/makepad/makepad makepad 
git clone https://github.com/makepad/makepad makepad/edit_repo 
cd makepad 
cargo run -p makepad --release 
```
