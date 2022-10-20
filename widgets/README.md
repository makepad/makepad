# makepad-widgets

## Overview

This is the top-level crate for Makepad Framework, a next-generation UI framework for Rust. Applications built using Makepad Framework can run both natively and on the web, are rendered entirely on the GPU, and support a novel feature called live design.

Live design means that Makepad Framework provides the infrastructure for other applications, such as an IDE, to hook into your application and change its design while your application keeps running. To facilitate this, the styling of Makepad Framework applications is described using a DSL. Code written in this DSL is tightly integrated with the main Rust code via the use of proc macros.

An IDE that is live design aware detects when changes are made in DSL code rather than in Rust code, so that instead of triggering a full recompilation, it can send the changes to the DSL code over to the application, allowing the latter to update itself. (The [makepad-studio](https://crates.io/crates/makepad-studio) crate contains a working prototype of an IDE that will eventually be capable of this, but it is still under heavy development.)

This crate contains a collection of basic widgets that almost every application needs. At the moment of this writing, the following widgets are supported:

- Windows
- Dropdown menus
- Docks
- Splitters
- Tab bars
- Frames
- Scrollbars
- File trees
- Labels
- Buttons
- Checkboxes
- Radio buttons
- Color pickers

In addition to these widgets, this crate also contains re-exports of two lower level crates, namely [makepad-draw-2d](https://crates.io/crates/makepad-draw-2d), which contains all code related to drawing applications, and [makepad-platform](https://crates.io/crates/makepad-platform), which contains all platform specific code. Finally, it contains a collection of base fonts.

In short, to build an application in Makepad Framework, most of the time this crate is the only one you'll need.

## Caveats

Although Makepad Framework is complete enough that you can write your own applications with it, it is still under heavy development. At the moment, we only support Mac and web, (though we intend to add support for Windows and Linux very soon). We also make no guarantees at this point with respect to API stability. Please keep that in mind should you decide to use Makepad Framework for your own applications. Finally, we still lack significant functionality in the areas of font rendering, internationalization, etc.

## Examples

### Simple Example

A very simple example, consisting of window with a button and a counter, can be found in the [makepad-example-simple](https://crates.io/crates/makepad-example-simple) crate. It's fairly well commented, so this should be a good starting point for playing around with Makepad Framework.

### Ironfish (Electronic Synthesizer)

A much more impressive example can be found in the [makepad-example-ironfish](https://crates.io/crates/makepad-example-ironfish) crate. Ironfish is an electronic synthesizer written entirely in Makepad Framework. If you want to get an idea of the kind of applications you could build with Makepad Framework, this is the example for you.

## Contact

If you have any questions/suggestions, feel free to reach out to us on our discord channel:
https://discord.com/invite/urEMqtMcSd=