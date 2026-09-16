# makepad-android-state

This crate is responsible for holding Makepad's Android-specific context states.
It exists solely to allow external crates to access those Android states
without depending on the entirety of Makepad.

These two states are:
1. The JavaVM instance initialized by the JNI layer.
  * This cannot be set by foreign code outside this crate,
    as it is only ever set once during the lifetime of the app process.
2. The current Makepad Activity instance.
  * This *can* be set by foreign code outside this crate,
    as the underlying Android platform may tear down and reconstruct
    the activity instance multiple times during the app's lifetime.
  * However, for safety reasons, we only permit a single caller
    to obtain the private "set_activity" function, which ensures that
    only the internal Makepad framework can set the activity instance.

## Usage
> Note: you probably want to use the [`robius-android-env`] crate instead of using this crate directly, or an even higher-level crate that depends on [`robius-android-env`].

External users of this crate should only care about two functions:
1. [`get_java_vm()`]: returns a pointer to the JavaVM instance,
   through which you can obtain the JNI environment.
2. [`get_activity()`]: returns a pointer to the current Makepad Activity instance.

All other functions are intended for Makepad-internal use only,
and will not be useful for external users.

[`robius-android-env`]: https://github.com/project-robius/robius-android-env
