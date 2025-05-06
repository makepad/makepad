Makepad android quick build example for Macos, Linux-x64 and Windows.
Please plug an android device set to developer mode into your PC via USB. The toolchain install is completely local and does not change things in your path. After compiling/running you have to give the app some rights otherwise it cant access midi e.d. 

## Install toolchain

Run the following command to install needed tools for building the applications APK

```
cargo run -p cargo-makepad --release -- android toolchain-install
```

In case you need to build for arquitectures different from 64-bit ARM, you can specify different ABI options using the --target option. You can install toolchains for all supported ABI using the following command:

```
cargo run -p cargo-makepad --release -- android --target=all toolchain-install
```

## Build and run applications

For instance, let's build the Ironfish example application

```
cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
```

You can also customize the package name and application label

```
cargo run -p cargo-makepad --release -- android --package-name=com.yourcompany.myapp --app-label="My Example App" run -p makepad-example-ironfish
```
