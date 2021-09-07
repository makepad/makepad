# Introducing Makepad

Makepad is a creative software development platform built around Rust. We aim to make the creative software development process as fun as possible! To do this we will provide a set of visual design tools that modify your application in real time, as well as a library ecosystem that allows you to write highly performant multimedia applications. 

The Makepad development platform and library ecosystem are MIT licensed, and will be available for free as part of Makepad Basic. In the near future, we will also introduce Makepad Pro, which will be available as a subscription model. Makepad Pro will include the visual design tools. Because the library ecosystem is MIT licensed, all applications made with the Pro version are entirely free licensed.

Today, we launch an early alpha of Makepad Basic. This version shows off the development platform, but does not include the visual design tools or library ecosystem yet. Today it effectively is a GPU accelerated Rust editor. However since we are developing makepad in makepad for months already, we released what we have now as a starting point. Although Makepad is primarily a native application, its UI is perfectly capable of running on the web. The web build can be tried here http://makepad.dev Try browsing the source code and pressing alt in a large code file! To compile code yourself, you have to install the native version. Right now makepad is set up to compile a simple WASM example you run in a browser from a localhost url.

# How to use

After install (see below) you can open the following file in makepad, and when you change the rust code, the browser should live reload the wasm application as you type.

open this file the makepad editor UI: 
```
main/makepad/examples/webgl_example_wasm/src/sierpinski.rs
```
open this url in your browser: 
http://127.0.0.1:8000/makepad/examples/webgl_example_wasm/

# How to install

On all platforms first install Rust. We have seen the gnu chain fail a lot on windows, so if you are up for it also have to install msvc.

https://www.rust-lang.org/tools/install

# MacOS

```
git clone https://github.com/makepad/makepad
cd makepad
tools/macos_rustup.sh
cargo run -p makepad --release
```

# Windows

Clone this repo using either gitub desktop or commandline: https://github.com/makepad/makepad
Open a cmd.exe in the directory you just cloned. Gh desktop makes: Documents\\Github\\makepad

Gnu chain (can fail):
```
rustup default stable-gnu
tools/windows_rustup.bat
cargo run -p makepad --release
```

MSVC chain (install msvc first):
```
rustup default stable-msvc
tools/windows_rustup.bat
cargo run -p makepad --release
```


# Linux
```
git clone https://github.com/makepad/makepad
cd makepad
tools/linux_rustup.sh
cargo run -p makepad --release
```

# Troubleshooting
```
Delete old settings unix: rm *.ron
Delete old settings windows: del *.ron
Make sure you are on master: git checkout master
Update rust: rustup update
Make sure you have wasm: rustup target add wasm32-unknown-unknown
Pull the latest: git pull
```

Still have a problem? Report here: https://github.com/makepad/makepad/issues
