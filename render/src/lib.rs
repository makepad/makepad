#![allow(dead_code)]

#[macro_use]
mod cx;
#[macro_use]
mod livemacros;

#[cfg(all(not(feature="ipc"),target_os = "linux"))]
mod cx_opengl;
#[cfg(all(not(feature="ipc"),target_os = "linux"))]
mod cx_xlib;
#[cfg(all(not(feature="ipc"),any(target_os = "linux")))]
mod cx_linux;

#[cfg(all(not(feature="ipc"),target_os = "macos"))]
mod cx_metal;
#[cfg(all(not(feature="ipc"),target_os = "macos"))]
mod cx_cocoa;
#[cfg(all(not(feature="ipc"),any(target_os = "macos")))]
mod cx_macos;
#[cfg(all(not(feature="ipc"),any(target_os = "macos")))]
mod cx_apple;

#[cfg(all(not(feature="ipc"),target_os = "windows"))]
mod cx_dx11;
#[cfg(all(not(feature="ipc"),target_os = "windows"))]
mod cx_win32;
#[cfg(all(not(feature="ipc"),any(target_os = "windows")))]
mod cx_windows;

#[cfg(all(not(feature="ipc"),target_arch = "wasm32"))]
mod cx_webgl;
#[macro_use]
#[cfg(all(not(feature="ipc"),target_arch = "wasm32"))]
mod cx_wasm32;

#[macro_use]
#[cfg(all(not(feature="ipc"),any(target_os = "linux", target_os="macos", target_os="windows")))]
mod cx_desktop;

mod cx_style;

mod turtle;
mod fonts;
mod cursor;
mod window;
mod view;
mod pass;
mod texture;
mod animator;
mod elements;
mod area;
mod geometrygen;

mod drawquad;
mod drawtext;
mod drawcolor;
mod drawcube;
mod drawimage;
mod events;
mod menu; 
mod geometry;
mod shader;
mod shader_std;
mod gpuinfo;

pub use crate::cx::*;
pub use crate::drawquad::*;
pub use crate::drawtext::*;
pub use crate::drawcolor::*;
pub use crate::drawcube::*;
pub use crate::drawimage::*;

pub use crate::elements::*;

use std::time::{Instant};

impl Cx{
    pub fn profile_start(&mut self, id:u64){
        self.profiles.insert(id, Instant::now());
    }
    
    pub fn profile_end(&self, id:u64){
        if let Some(inst) = self.profiles.get(&id){
            log!("Profile {} time {}", id, inst.elapsed().as_millis());
        }
        
    }
}
