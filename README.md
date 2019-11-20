# Introducing Makepad

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications.

Today, we launch an early alpha of Makepad Basic. This version shows off the development platform, but does not include the visual design tools or library ecosystem yet. It is intended as a starting point for feedback from you! Although Makepad is primarily a native application, its UI is perfectly capable of running on the web. If you want to get a taste of what Makepad looks like, play around with the web version, see it at http://makepad.github.io/ To compile code, you have to install the native version.

The Makepad development platform and library ecosystem are MIT licensed, and will be available for free as part of Makepad Basic. In the near future, we will also introduce Makepad Pro, which will be available as a subscription model. Makepad Pro will include the visual design tools, and and live VR environment. Because the library ecosystem is MIT licensed, all applications made with the Pro version are entirely free licensed.

Install Makepad locally so you can compile code:

```
git clone https://github.com/makepad/makepad makepad
git clone https://github.com/makepad/makepad makepad/edit_repo
cd makepad
cargo run -p makepad --release
```
