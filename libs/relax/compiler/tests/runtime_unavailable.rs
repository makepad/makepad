//! rustc can check the fixture UIs, but only the Relax host supplies sandboxed modules.
//! Refuse loads explicitly; never substitute the native oracle for runtime code.
#![allow(dead_code)]

const UNAVAILABLE: &str = "run-time modules require makepad run; run this fixture with makepad run";

#[derive(Clone, Copy)]
pub struct Module { _private: () }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edit { Patched, NeedsReload }
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream { Applied { kept: usize }, Incomplete, Pending, NeedsReload }

impl Module {
    pub fn load_source(_: &str, _: &str, _: &str, _: &[&str]) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn load_stream(_: &str, _: &str, _: &str, _: &[&str]) -> Result<Self, String> {
        Err(UNAVAILABLE.into())
    }
    pub fn get<T>(self, _: &str) -> Result<T, String> { Err(UNAVAILABLE.into()) }
    pub fn stream(self, _: &str) -> Result<Stream, String> { Err(UNAVAILABLE.into()) }
    pub fn finish_stream(self, _: &str) -> Result<Edit, String> { Err(UNAVAILABLE.into()) }
    pub fn unload(self) {}
}

pub fn guard<T>(_: u64, _: impl FnOnce() -> T) -> Result<T, String> {
    Err(UNAVAILABLE.into())
}
