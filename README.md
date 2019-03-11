# makepad
Livecoding Rust for 2D CAD

This repo is for developing Makepad, a livecoding Rust IDE for 2D CAD design. The goal of the containing code is to be a Code Editor / UI kit for the Makepad application and will change without notice to suit that goal. The makepad application itself may be closed-sourced and sold, since we also have to feed our families.

The vision is to build a livecoding / design hybrid program, where procedural design and code are fused in one environment. Useable for learning, creating, designing, and just plain fun. However  building a performant-enough UI thats also scalable in a development sense has been a serious problem. We've invested over a decade of trying to build this, one way or another in C++, JS, HTML, WebGL in every possible combination and variation. We looked at React, imGUI, Elm and any UI kit we could find to look for solutions. However everyone always neatly skipped the rendering part and left it to HTML (imgui did its own tho) and chose solutions that were limited by the underlying renderstack (or host-language) performance (react). This UI stack has a unique way of approaching it we'll dub 'dual immediate mode' and uses multi-platform shaders for styling that are compiled from Rust source via a proc macro. Rust is extremely helpful in managing complexity, its much better as a development experience than anything before so we are hopeful this time it will work.

The aim of this toolkit is NOT to be the end all be all of UI kits as a standalone library for everyone. We are not a giant company with huge teams to maintain this project specifically. The goal is to build a livecoding IDE and designtools that don't suck or fall to pieces along the way of complexity.

However since openness is fine and this may be useful to someone, the foundational technologies (UI/rendering/vector engine) will be MIT opensource, but provided AS IS.

We are not accepting pull requests or feature requests or bug reports or documenting the code.
You may use, reuse, build a giant opensource community on your fork, whatever you feel like. But our focus is code/designtools and these libraries are just a tool for us to get there. Also, since this is attempt #11 it may still entirely fail and will be dropped completely. As has been done before many times. Be warned!

Have fun programming!