makepad-android
===============

This is the skeleton of a minimal embedding of Makepad for Android. The project
structure was created by creating a new project in Android Studio with an empty
activity, and then only keeping the app/src/main directory.

To be able to build, install, and run the application, the following environment 
variables should be defined:

- ```ANDROID_HOME```
  
  This points to the Java SDK. You can install the Java SDK with Android Studio.
  On my system, the SDK gets installed to ~/Library/Android/sdk

- ```NDK_HOME```
  
  This points to the Java NDK. You can install the Java NDK with Android Studio.
  On my system, the NDK gets installed to $ANDROID_HOME/ndk/<version>. I've
  tested the project with NDK version 25.1.8937393.

- ```JAVA_HOME```
  
  This points to the Java Runtime Environment (JRE). The JRE does need to be
  installed separately, but instead is part of Android Studio. On my system, the
  JRE can be found at /Applications/Android Studio.app/Contents/jre/Contents/Home.

In addition to these environment variables, the build tools and platform tools
of the SDK should be added to your path:

```
export PATH=$PATH:$ANDROID_HOME/build-tools/33.0.1
export PATH=$PATH:$ANDROID_HOME/platform-tools
```

Finally, you need to update the paths in `rust/.cargo/config` to refer to your
version of the NDK. It would have been nicer if we could have just used
`NDK_HOME` here, but unfortunately it seems that Cargo does not expand
environment variables.

Once you've set up your environment correctly, you should be able to build,
install, and run the application. To assist with this, I've provided the
following shell scripts:

- `build.sh`: builds the application.
- `install.sh`: installs the application to your phone.
- `start.sh`: starts running the application on your phone.
- `stop.sh`: stops running the application on your phone.
- `uninstall.sh`: uninstalls the application from your phone.

Should you later want to run the application in Android Studio again, you have to create a new project with an empty activity, replace the contents of its `app/src/main` directory with the contents of this directory, and then make sure the Rust library can be found in `app/src/main/jniLibs/arm64-v8a/` (which is where Gradle looks for it).