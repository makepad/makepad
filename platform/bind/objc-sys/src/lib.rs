
mod malloc_buf;

pub use encode::{Encode, EncodeArguments, Encoding};
pub use message::{Message, MessageArguments, MessageError};

pub use message::send_message as __send_message;
pub use message::send_super_message as __send_super_message;

#[macro_use]
mod macros;

pub mod runtime;
pub mod declare;
pub mod rc;
mod encode;
mod message;

//#[cfg(test)]
//mod test_utils;
