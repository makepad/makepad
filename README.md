# Introducing Makepad

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications.

As we're working towards our first public alpha version, you'll be able to see our final steps towards it here. The alpha version of Makepad will show off the development platform, but does not include the visual design tools or library ecosystem yet.

What features we have now:
- Native compiles to linux, windows, macos
- Compiles to Wasm for demo purposes (no compiler integration, no backend)
- Rust Compiler integration with errors/warnings in editor
- Virtual viewport log viewer that can take infinite loop printlns
- Code editor with live code folding (press alt)
- Dock panel system / filetree
- Workspaces (for file access/builds) with networking support
- Built in HTTP server with livereload for wasm development

Install makepad locally so you can compile code: 

```
git clone https://github.com/makepad/makepad makepad 

git clone https://github.com/makepad/makepad makepad/edit_repo 

cd makepad 

cargo run -p makepad --release 
```
