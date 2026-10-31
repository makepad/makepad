//! The Metal side of the media module: where decoded and captured frames become HAL textures.

use super::resources::Tex;
use crate::backend::apple::media::{external_texture, Sink};
use super::device::Device;
use crate::ledger::{Kind, LedgerEntry};
use crate::session::Session;
use crate::types::*;
use makepad_apple_sys::*;
use std::any::Any;
use crate::FastMap;

/// The texture storage of the Metal backend, as the media module sees it.
pub struct MetalSink<'a> {
    pub dev: &'a Device,
    pub tex: &'a mut FastMap<(u32, u32), Tex>,
    pub now: u64,
}

impl MetalSink<'_> {
    fn set_row(&self, s: &mut Session, id: Id, fmt: u32, w: u32, h: u32, bytes: u64) {
        let (now, frame) = (self.now, s.frame);
        match s.ledger.get_mut(Kind::Texture, id.index).filter(|l| l.gen == id.gen) {
            Some(l) => {
                l.bytes = bytes;
                l.format = fmt;
                l.width = w;
                l.height = h;
            }
            None => s.ledger.add(LedgerEntry {
                kind: Kind::Texture,
                detail: "video frame",
                index: id.index,
                gen: id.gen,
                bytes,
                format: fmt,
                width: w,
                height: h,
                mips: 1,
                attr: Attr::default(),
                created_ns: now,
                created_frame: frame,
                last_used_frame: 0,
                last_used_ns: 0,
            }),
        }
    }
}

impl Sink for MetalSink<'_> {
    fn device(&self) -> ObjcId {
        self.dev.device
    }

    fn declare(&mut self, s: &mut Session, id: Id, fmt: u32) {
        if s.textures.get(s.id, id).is_ok() {
            return;
        }
        if s.textures.insert(s.id, id, s.id, external_texture(fmt, 1, 1)).is_ok() {
            self.set_row(s, id, fmt, 0, 0, 0);
        } else {
            s.stale();
        }
    }

    fn adopt(&mut self, s: &mut Session, id: Id, mtl: ObjcId, w: u32, h: u32, fmt: u32, keep: Option<Box<dyn Any + Send>>) -> bool {
        if mtl.is_null() {
            return false;
        }
        let Ok(entry) = s.textures.get_mut(s.id, id) else {
            // the app destroyed the texture (or made a newer one of the slot): the frame has no home
            s.stale();
            return false;
        };
        let bytes = unsafe { msg_send![mtl, allocatedSize] };
        entry.val = external_texture(fmt, w, h);
        entry.val.bytes = bytes;
        let tex = Tex {
            gen: id.gen,
            obj: RcObjcId::from_unowned(NonNull::new(mtl).expect("checked")),
            surface: None,
            surface_id: 0,
            last_bound: 0,
            _keep: keep,
        };
        if let Some(old) = self.tex.insert((s.id, id.index), tex) {
            self.dev.retire(Box::new(old));
        }
        self.set_row(s, id, fmt, w, h, bytes);
        true
    }

    fn dummy_plane(&mut self, s: &mut Session, id: Id) -> bool {
        unsafe {
            let descriptor: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![descriptor, setTextureType: MTLTextureType::D2];
            let _: () = msg_send![descriptor, setWidth: 1u64];
            let _: () = msg_send![descriptor, setHeight: 1u64];
            let _: () = msg_send![descriptor, setDepth: 1u64];
            let _: () = msg_send![descriptor, setPixelFormat: MTLPixelFormat::R8Unorm];
            let _: () = msg_send![descriptor, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![descriptor, setUsage: MTLTextureUsage::ShaderRead];
            let tex: ObjcId = msg_send![self.dev.device, newTextureWithDescriptor: descriptor];
            let _: () = msg_send![descriptor, release];
            if tex.is_null() {
                return false;
            }
            let ok = self.adopt(s, id, tex, 1, 1, format::R8, None);
            // `adopt` retained it; the creation reference goes
            let _: () = msg_send![tex, release];
            ok
        }
    }

    fn drop_texture(&mut self, s: &mut Session, id: Id) {
        if self.tex.get(&(s.id, id.index)).is_some_and(|t| t.gen == id.gen) {
            if let Some(old) = self.tex.remove(&(s.id, id.index)) {
                self.dev.retire(Box::new(old));
            }
            if let Some(l) = s.ledger.get_mut(Kind::Texture, id.index).filter(|l| l.gen == id.gen) {
                l.bytes = 0;
            }
        }
    }
}
