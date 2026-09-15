#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::OH_NativeXComponent;

/** @brief Defines the expected frame rate range struct.

@since 11
@version 1.0*/
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct OH_NativeXComponent_ExpectedRateRange {
    /// The minimum frame rate of dynamical callback rate range.
    pub min: i32,
    /// The maximum frame rate of dynamical callback rate range.
    pub max: i32,
    /// The expected frame rate of dynamical callback rate range.
    pub expected: i32,
}

extern "C" {
    /** @brief Set the Expected FrameRateRange.

    @param component Indicates the pointer to this <b>OH_NativeXComponent</b> instance.
    @param callback Indicates the pointer to a expected rate range.
    @return Returns the status code of the execution.
    @since 11
    @version 1.0*/
    pub fn OH_NativeXComponent_SetExpectedFrameRateRange(
        component: *mut OH_NativeXComponent,
        range: *mut OH_NativeXComponent_ExpectedRateRange,
    ) -> i32;

    /** @brief Registers a callback for this <b>OH_NativeXComponent</b> instance.

    @param component Indicates the pointer to this <b>OH_NativeXComponent</b> instance.
    @param callback Indicates the pointer to a onFrame callback.
    @return Returns the status code of the execution.
    @since 11
    @version 1.0*/
    pub fn OH_NativeXComponent_RegisterOnFrameCallback(
        component: *mut OH_NativeXComponent,
        callback: ::core::option::Option<
            unsafe extern "C" fn(
                component: *mut OH_NativeXComponent,
                timestamp: u64,
                targetTimestamp: u64,
            ),
        >,
    ) -> i32;

    /** @brief UnRegister a callback for this <b>OH_NativeXComponent</b> instance.

    @param component Indicates the pointer to this <b>OH_NativeXComponent</b> instance.
    @return Returns the status code of the execution.
    @since 11
    @version 1.0*/
    pub fn OH_NativeXComponent_UnregisterOnFrameCallback(
        component: *mut OH_NativeXComponent,
    ) -> i32;
}
