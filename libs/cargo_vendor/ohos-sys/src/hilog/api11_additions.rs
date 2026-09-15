#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::{LogLevel, LogType};

/** @brief Defines the function pointer type for the user-defined log processing function.

@param type Indicates the log type. The type for third-party applications is defined by {@link LOG_APP}.
@param level Indicates the log level, which can be <b>LOG_DEBUG</b>, <b>LOG_INFO</b>, <b>LOG_WARN</b>,
<b>LOG_ERROR</b>, and <b>LOG_FATAL</b>.
@param domain Indicates the service domain of logs. Its value is a hexadecimal integer ranging from 0x0 to 0xFFFF.
@param tag Indicates the log tag, which is a string used to identify the class, file, or service behavior.
@param msg Indicates the log message itself, which is a formatted log string.
@since 11*/
pub type LogCallback = ::core::option::Option<
    unsafe extern "C" fn(
        type_: LogType,
        level: LogLevel,
        domain: ::core::ffi::c_uint,
        tag: *const ::core::ffi::c_char,
        msg: *const ::core::ffi::c_char,
    ),
>;
extern "C" {
    /** @brief Set the user-defined log processing function.

    After calling this function, the callback function implemented by the user can receive all hilogs of the
    current process.
    Note that it will not change the default behavior of hilog logs of the current process, no matter whether this
    interface is called or not. \n

    @param callback Indicates the callback function implemented by the user. If you do not need to process hilog logs,
    you can transfer a null pointer.
    @since 11*/
    pub fn OH_LOG_SetCallback(callback: LogCallback);
}
