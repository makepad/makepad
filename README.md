# Introducing Makepad

Makepad is a new VR, web and native collaborative shader programming environment. 
It will support many different shader modes including many vertex-shaders 
besides the well known shader toy SDF programs. This makes shader coding possible
for more compute constrained environments like VR goggles or mobiles.
Try makepad now on a Quest in the quest browser, click the goggles top right of the UI. Try touching the leaves of the tree with your hands! Magic!

https://makepad.dev

The Makepad development platform and library ecosystem are MIT licensed,
for the Quest and in the future iOS we will provide paid, native version

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
