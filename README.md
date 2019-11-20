# Introducing Makepad

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications.

As we're working towards our first public alpha version, you'll be able to see our final steps towards it here. The alpha version of Makepad will show off the development platform, but does not include the visual design tools or library ecosystem yet. Right now we are still working on CI for all platforms and tests, so if it fails to run check back soon or let us know if it takes too long.

The Makepad development platform and library ecosystem are MIT licensed, and will be available for free as part of Makepad Basic. In the near future, we will also introduce Makepad Pro, which will be available as a subscription model. Makepad Pro will include the visual design tools, and and live VR environment. Because the library ecosystem is MIT licensed, all applications made with the Pro version are entirely free licensed.

Makepad currently has the following features:
- Compiles natively to Linux, MacOS, and Windows.
- Compiles to WebAssembly for demo purposes (see caveats below).
- Built-in HTTP server with live reload support for WebAssembly development.
- Code editor with live code folding (press alt).
- Log viewer with a virtual viewport, that can handle printlns in an infinite loop.
- Dock panel system / file tree.
- Rust compiler integration, with errors/warning in the IDE.

If you're interested in Makepad, you can check out the web build here:
https://makepad.github.io/

Note that the web build of Makepad does not feature any compiler integration. If you want to be able to compile code, you have to install Makepad locally.

To install Makepad locally, run the following commands:
```
git clone https://github.com/makepad/makepad makepad 
git clone https://github.com/makepad/makepad makepad/edit_repo 
cd makepad 
cargo run -p makepad --release 
```
