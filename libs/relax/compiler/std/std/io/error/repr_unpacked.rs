//! Enum representation for targets whose pointers cannot hold the 64-bit encoding.

use alloc::boxed::Box;

use super::{Custom, ErrorData, ErrorKind, RawOsError, SimpleMessage};

pub(super) struct Repr(ErrorData<Box<Custom>>);

impl Repr {
    #[inline]
    pub(super) fn new_custom(custom: Box<Custom>) -> Self {
        Self(ErrorData::Custom(custom))
    }

    #[inline]
    pub(super) fn new_os(code: RawOsError) -> Self {
        Self(ErrorData::Os(code))
    }

    #[inline]
    pub(super) fn new_simple(kind: ErrorKind) -> Self {
        Self(ErrorData::Simple(kind))
    }

    #[inline]
    pub(super) const fn new_simple_message(message: &'static SimpleMessage) -> Self {
        Self(ErrorData::SimpleMessage(message))
    }

    #[inline]
    pub(super) fn data(&self) -> ErrorData<&Custom> {
        match &self.0 {
            ErrorData::Os(code) => ErrorData::Os(*code),
            ErrorData::Simple(kind) => ErrorData::Simple(*kind),
            ErrorData::SimpleMessage(message) => ErrorData::SimpleMessage(*message),
            ErrorData::Custom(custom) => ErrorData::Custom(&**custom),
        }
    }

    #[inline]
    pub(super) fn data_mut(&mut self) -> ErrorData<&mut Custom> {
        match &mut self.0 {
            ErrorData::Os(code) => ErrorData::Os(*code),
            ErrorData::Simple(kind) => ErrorData::Simple(*kind),
            ErrorData::SimpleMessage(message) => ErrorData::SimpleMessage(*message),
            ErrorData::Custom(custom) => ErrorData::Custom(&mut **custom),
        }
    }

    #[inline]
    pub(super) fn into_data(self) -> ErrorData<Box<Custom>> {
        self.0
    }
}
