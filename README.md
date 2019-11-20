# Introducing Makepad

Alpha release soon, as its all public you see our final steps towards it here. Not just fully ready yet, but will be soon.
The alpha release will first target developing commandline rust applications, as this is what we use makepad for ourselves (dogfooding makepad). Essentially makepad as it will be for the next few months is a a compile-yourself (in 2 minutes) local Rust IDE that has a very tight live copmile cycle with in-editor errors and std-out logviewer.
The fancy live visual editing we estimate at end Q1 '20.

Our feature todo list for this alpha:

Pre alpha (what is here now essentially)
- Homepage in application

Alpha
- File/new file menu and find replace
- Documentation how to work with your own projects in makepad via the settings

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications. 

Today, we launch an early alpha of Makepad Basic. This version shows off the development platform, but does not include the visual design tools or library ecosystem yet. It is intended as a starting point for feedback from you! Although Makepad is primarily a native application, its UI is perfectly capable of running on the web. If you want to get a taste of what Makepad looks like, play around with the web version, see it at http://makepad.github.io/ To compile code, you have to install the native version. 

The Makepad development platform and library ecosystem are MIT licensed, and will be available for free as part of Makepad Basic. In the near future, we will also introduce Makepad Pro, which will be available as a subscription model. Makepad Pro will include the visual design tools, and and live VR environment. Because the library ecosystem is MIT licensed, all applications made with the Pro version are entirely free licensed. 

Install makepad locally so you can compile code: 

```
git clone https://github.com/makepad/makepad makepad 

git clone https://github.com/makepad/makepad makepad/edit_repo 

cd makepad 

cargo run -p makepad --release 
```
