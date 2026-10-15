//! Services a program registers on its process's endpoint through a C ABI: the same three
//! functions whether the endpoint is the process's own (a native app, makepad-platform) or the
//! Relax host's (a program under `makepad run`). One channel = one producer: `open` gets the OPEN
//! arguments and a sink, answers through the sink and ends the channel with its last sink call;
//! `close` tells the producer the client went away.

use crate::conn::{CLOSE_NORMAL, CLOSE_REFUSED};
use crate::endpoint::Endpoint;
use crate::ChannelTx;
use std::sync::{Arc, Mutex};

/// `sink(ctx, kind, ptr, len)` of a service channel: kind 0 sends `ptr..len` as one message (false:
/// the client is gone), 1 ends the stream, 2 ends it with `ptr..len` as the reason. The end call
/// is the last the producer makes for the channel.
pub type ServiceSink = extern "C" fn(ctx: *mut std::ffi::c_void, kind: u32, ptr: *const u8, len: usize) -> bool;
/// Starts a producer for one channel (`args` = the OPEN arguments); returns its handle, 0 when
/// refused (after an end call with the reason).
pub type ServiceOpen = extern "C" fn(args: *const u8, len: usize, sink: ServiceSink, ctx: *mut std::ffi::c_void) -> usize;
/// The client closed the channel: the producer for `handle` stops.
pub type ServiceClose = extern "C" fn(handle: usize);

struct ProgramChannel {
    tx: ChannelTx,
    reason: Mutex<Option<String>>,
}

extern "C" fn sink(ctx: *mut std::ffi::c_void, kind: u32, ptr: *const u8, len: usize) -> bool {
    let channel = unsafe { &*(ctx as *const ProgramChannel) };
    let bytes = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(ptr, len) } };
    if kind == 0 {
        return channel.tx.send(bytes.to_vec()).is_ok();
    }
    // the end: close the channel and give back the producer's reference
    let why = String::from_utf8_lossy(bytes).into_owned();
    if kind == 2 {
        *channel.reason.lock().unwrap() = Some(why.clone());
    }
    channel.tx.close(if kind == 2 { CLOSE_REFUSED } else { CLOSE_NORMAL }, &why);
    drop(unsafe { Arc::from_raw(ctx as *const ProgramChannel) });
    true
}

impl Endpoint {
    /// Serves `name` with a program's producer functions.
    pub fn register_program(&self, name: &str, version: u32, open: ServiceOpen, close: ServiceClose) {
        self.register(
            name,
            version,
            Arc::new(move |conn, opened| {
                let channel = Arc::new(ProgramChannel { tx: opened.channel.tx(), reason: Mutex::new(None) });
                // one reference for the producer (freed by its end call), one kept here
                let for_producer = Arc::into_raw(channel.clone()) as *mut std::ffi::c_void;
                let handle = open(opened.args.as_ptr(), opened.args.len(), sink, for_producer);
                if handle == 0 {
                    let why = channel.reason.lock().unwrap().take().unwrap_or_else(|| "the program refused".into());
                    return Err((CLOSE_REFUSED, why));
                }
                let ch = opened.channel;
                let hooks = conn.hooks();
                let _ = std::thread::Builder::new().name("makepad-remote-service".into()).spawn(move || {
                    (hooks.enter)();
                    while let Some(ev) = ch.recv() {
                        if matches!(ev, crate::Event::Closed(..)) {
                            break;
                        }
                    }
                    close(handle);
                    (hooks.exit)();
                });
                Ok(())
            }),
        );
    }
}
