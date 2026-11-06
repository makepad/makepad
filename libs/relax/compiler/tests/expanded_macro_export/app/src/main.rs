#[macro_use]
extern crate xmx_lib;

use xmx_lib::third as imported;

macro_rules! app_make {
    ($name:ident, $v:expr) => {
        #[macro_export]
        macro_rules! $name {
            () => {
                $v
            };
        }
    };
}

macro_rules! app_local {
    ($name:ident, $v:expr) => {
        macro_rules! $name {
            () => {
                $v
            };
        }
    };
}

#[macro_use]
mod app_defs;
#[macro_use]
mod app_exports;
mod app_uses;

mod via_glob {
    use xmx_lib::*;
    pub fn get() -> u32 {
        answer!()
    }
}

fn main() {
    println!("expanded macros {} {} {} {} {} {} | {} {}", xmx_lib::answer!(), other!(), imported!(), via_glob::get(), xmx_lib::inner(), xmx_lib::from_file!(), app_file!(), app_uses::get());
}
