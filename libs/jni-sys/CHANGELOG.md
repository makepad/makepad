# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2023-09-25

### Added

- Added `JNI_VERSION_9`, `JNI_VERSION_10`, `JNI_VERSION_19`, `JNI_VERSION_20` and `JNI_VERSION_21` constants
- Added `GetModule()` to `JNINativeInterface` ([#22](https://github.com/jni-rs/jni-sys/pull/22))
- `IsVirtualThread()` to `JNINativeInterface` ([#32](https://github.com/jni-rs/jni-sys/pull/32))
- Implemented `Debug` trait for all types ([#31](https://github.com/jni-rs/jni-sys/pull/31))
- Added support for `no_std` environments ([#12](https://github.com/jni-rs/jni-sys/pull/12))

### Changed

- `jboolean` is now an alias for `bool` instead of `u8` ([#23](https://github.com/jni-rs/jni-sys/pull/23))
- The `JNIInvokeInterface_` and `JNINativeInterface_` structs were turned into unions that namespace functions by version ([#28](https://github.com/jni-rs/jni-sys/pull/28)):

    This makes it much clearer what version of JNI you require to access any function safely.

    So instead of a struct like:

    ```rust
    struct JNINativeInterface_ {
        pub reserved0: *mut c_void,
        ..
        pub GetVersion: unsafe extern "system" fn(env: *mut JNIEnv) -> jint,
        ..
        pub NewLocalRef: unsafe extern "system" fn(env: *mut JNIEnv, ref_: jobject) -> jobject,
    }
    ```

    there is now a union like:

    ```rust
    union JNINativeInterface_ {
        v1_1: JNINativeInterface__1_1,
        v1_2: JNINativeInterface__1_2,
        reserved: JNINativeInterface__reserved,
    }
    ```

    And you can access `GetVersion` like: `env.v1_1.GetVersion` and access `NewLocalRef` like: `env.v1_2.NewLocalRef`.

    Each version struct includes all functions for that version and lower, so it's also possible to access `GetVersion` like `env.v1_2.GetVersion`.

- Function pointers are no longer wrapped in an `Option<>` ([#25](https://github.com/jni-rs/jni-sys/pull/25))

## [0.3.0] - 2017-07-20

### Changed

- Changed jvalue into a union

[unreleased]: https://github.com/jni-rs/jni-sys/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/jni-rs/jni-sys/compare/v0.2.5...v0.3.0
