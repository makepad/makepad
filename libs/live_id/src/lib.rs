// thread-locals take no pthread key each on Android (libs/thread_local)
#[macro_use(thread_local)]
extern crate makepad_thread_local;

pub use makepad_live_id_macros;
pub use makepad_live_id_macros::*;
//pub use makepad_error_log;

pub mod live_id;
pub use live_id::*;

