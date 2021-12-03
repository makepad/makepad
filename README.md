# Introducing Makepad Framework and Makepad Studio

Makepad Framework is a new web and native-rendering UI framework for Rust.
Makepad Studio is an IDE with integrated designtool to develop applications with Makepad Framework.

All code in this repository EXCEPT the files in studio/src/design_editor are licensed as MIT/Apache2.

This means the code editing part of Makepad Studio is licensed MIT/Apache2, and the visual designtooling is not.
For our commercial offering we are building a visual designer extension.

During the alpha/beta phase of the product development we keep the files for the visaul designer inside the OSS repository,
however after product launch we will distribute these in a different manner.

For the first build of our editor / UI you can look at the following URL in your browser,

https://makepad.dev

# How to install the native version

On all platforms first install Rust. 
We are currently relying on nightly because of the procmacro span information needed. Hopefully this will stabilise soon.

https://www.rust-lang.org/tools/install

# MacOS

```
git clone https://github.com/makepad/makepad
cd makepad
tools/macos_rustup.sh
cargo run -p makepad_studio --release
```

# Windows

Clone this repo using either gitub desktop or commandline: https://github.com/makepad/makepad
Open a cmd.exe in the directory you just cloned. Gh desktop makes: Documents\\Github\\makepad

```
tools/windows_rustup.bat
cargo run -p makepad_studio --release
```

# Linux
```
git clone https://github.com/makepad/makepad
cd makepad
tools/linux_rustup.sh
cargo run -p makepad_studio --release
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
