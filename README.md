# Introducing Makepad

Makepad is a new VR, web and native-rendering UI framework for Rust.
It utilises shaders as its styling primitives, and we are currently developing a live UI design and animation environment for it.

For the first build of our editor / UI you can look at the following URL in your browser,
or try makepad now on a Quest in the quest browser, click the goggles top right of the UI. Try touching the leaves of the tree with your hands! Magic!

https://makepad.dev

The Makepad Framework and UI component system is MIT licensed, our UI designer will be cloud based and commercial.

# How to install the native version

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
