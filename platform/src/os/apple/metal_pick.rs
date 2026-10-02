//! The Metal side of pick ids (`crate::pick`): a pick repaint draws every
//! pass that feeds the picked texture again with the pick variants into
//! the twins of its targets, deepest first, then copies the picked
//! texture's twin back.

use super::metal::{DrawPassMode, MetalCx};
use crate::cx::Cx;
use crate::draw_list::DrawListId;
use crate::draw_pass::DrawPassId;
use crate::makepad_objc_sys::objc_block;
use crate::os::apple::apple_sys::*;
use crate::pick::{PickError, PickImage, PickPaint, PickTicket};
use std::ptr::NonNull;
use std::sync::Mutex;

type PickResult = (u64, usize, usize, Vec<u8>);
thread_local! {
    static PICK_BUS: (crate::makepad_network::mpsc::SyncSender<PickResult>, crate::makepad_network::mpsc::Receiver<PickResult>) = crate::makepad_network::mpsc::sync_channel(8);
}

impl Cx {
    /// The images the GPU gave back since, into the results.
    pub(crate) fn poll_pick_results(&mut self) {
        let done: Vec<PickResult> = PICK_BUS.with(|(_, receive)| receive.try_iter().collect());
        for (ticket, width, height, bytes) in done {
            self.pick.results.push((PickTicket(ticket), Ok(PickImage { width, height, bytes })));
        }
    }

    /// The draw lists a pass draws, its sub-lists too.
    fn pick_lists(&self, list: DrawListId, out: &mut Vec<DrawListId>) {
        if out.contains(&list) || self.draw_lists.is_id_freed(list) {
            return;
        }
        out.push(list);
        let len = self.draw_lists[list].draw_item_order_len();
        for order in 0..len {
            let Some(item) = self.draw_lists[list].draw_item_id_at_order_index(order) else { continue };
            if let Some(sub) = self.draw_lists[list].draw_items[item].kind.sub_list() {
                self.pick_lists(sub, out);
            }
        }
    }

    /// `pass` after the passes it samples (depth first): what feeds it.
    fn pick_feeders(&self, pass: DrawPassId, out: &mut Vec<DrawPassId>, visiting: &mut Vec<DrawPassId>) {
        if out.contains(&pass) || visiting.contains(&pass) || self.passes.0.is_free(pass.0) {
            return;
        }
        let Some(list) = self.passes[pass].main_draw_list_id else { return };
        visiting.push(pass);
        let mut lists = Vec::new();
        self.pick_lists(list, &mut lists);
        let mut sampled: Vec<crate::texture::TextureId> = Vec::new();
        for list in &lists {
            let items = &self.draw_lists[*list].draw_items;
            for i in 0..items.len() {
                let Some(call) = items[i].kind.draw_call() else { continue };
                for t in call.texture_slots.iter().flatten() {
                    if !sampled.contains(&t.texture_id()) {
                        sampled.push(t.texture_id());
                    }
                }
            }
        }
        for texture in sampled {
            let producer = self.passes.id_iter().find(|p| {
                !self.passes.0.is_free(p.0) && self.passes[*p].color_textures.iter().any(|c| c.texture.texture_id() == texture)
            });
            if let Some(producer) = producer {
                self.pick_feeders(producer, out, visiting);
            }
        }
        visiting.pop();
        if !self.passes[pass].color_textures.is_empty() {
            out.push(pass);
        }
    }

    /// After a repaint: the first pick asked for, drawn. A pick whose
    /// pick pipelines are still compiling is drawn again on a later
    /// repaint.
    pub(crate) fn paint_picks(&mut self, metal_cx: &mut MetalCx) {
        self.poll_pick_results();
        if self.pick.requests.is_empty() {
            return;
        }
        let ticket = self.pick.requests[0].ticket;
        let texture = self.pick.requests[0].texture.texture_id();
        let target = self.passes.id_iter().find(|pass| {
            !self.passes.0.is_free(pass.0) && self.passes[*pass].color_textures.iter().any(|c| c.texture.texture_id() == texture)
        });
        let Some(target) = target else {
            self.pick.requests.remove(0);
            self.pick.results.push((ticket, Err(PickError::NotRendered)));
            return;
        };
        // Every pass that feeds the target: the passes drawing the textures
        // its draws sample, and theirs in turn; each before its readers.
        let mut passes: Vec<DrawPassId> = Vec::new();
        self.pick_feeders(target, &mut passes, &mut Vec::new());
        // The pick pipelines of every shader they draw, queued.
        let mut lists = Vec::new();
        for pass in &passes {
            if let Some(list) = self.passes[*pass].main_draw_list_id {
                self.pick_lists(list, &mut lists);
            }
        }
        let mut shaders: Vec<usize> = Vec::new();
        for list in &lists {
            let items = &self.draw_lists[*list].draw_items;
            for i in 0..items.len() {
                if let Some(call) = items[i].kind.draw_call() {
                    if !shaders.contains(&call.draw_shader_id.index) {
                        shaders.push(call.draw_shader_id.index);
                    }
                }
            }
        }
        crate::trace!("pick", "pick {}: {} passes, {} lists, {} shaders ({} with a pick variant)", ticket.0, passes.len(), lists.len(), shaders.len(), shaders.iter().filter(|i| self.draw_shaders.shaders[**i].mapping.pick_code.is_some()).count());
        for index in shaders {
            let sh = &self.draw_shaders.shaders[index];
            let (Some(os), Some(code)) = (sh.os_shader_id, sh.mapping.pick_code.clone()) else { continue };
            let written = sh.mapping.fragment_outputs;
            let shp = &mut self.draw_shaders.os_shaders[os];
            shp.ensure_pick(metal_cx, code, written);
        }
        // The pick: every pass into its twins, the flags of the real
        // passes left as they were.
        let kept: Vec<(DrawPassId, bool, u64)> = passes.iter().map(|id| (*id, self.passes[*id].paint_dirty, self.passes[*id].painted_serial)).collect();
        self.pick.paint = Some(PickPaint { ticket, target, texture, painted: Default::default(), not_ready: 0 });
        for pass in &passes {
            self.draw_pass(*pass, metal_cx, DrawPassMode::Pick);
            let painted: Vec<_> = self.passes[*pass].color_textures.iter().map(|c| c.texture.texture_id()).collect();
            if let Some(paint) = self.pick.paint.as_mut() {
                paint.painted.extend(painted);
            }
        }
        for (id, dirty, serial) in kept {
            self.passes[id].paint_dirty = dirty;
            self.passes[id].painted_serial = serial;
        }
        let paint = self.pick.paint.take().unwrap();
        crate::trace!("pick", "pick {}: {} targets drawn, {} draws still compiling", ticket.0, paint.painted.len(), paint.not_ready);
        if paint.not_ready == 0 {
            self.pick.requests.remove(0);
        } else {
            // Compiling: the next repaint draws it again.
            self.repaint_pass(target);
        }
    }

    /// The picked texture's twin, copied back once the pick's last pass
    /// (its own) has drawn, when every draw of it was ready.
    pub(crate) fn encode_pick_readback(&mut self, metal_cx: &MetalCx, pass: DrawPassId, command_buffer: ObjcId) {
        let Some(paint) = self.pick.paint.as_ref() else { return };
        if paint.target != pass || paint.not_ready > 0 {
            return;
        }
        let ticket = paint.ticket.0;
        let Some(twin) = self.pick.twins.get(&paint.texture) else { return };
        let cxtexture = &self.textures[twin.texture_id()];
        let (Some(alloc), Some(tex)) = (cxtexture.alloc.as_ref(), cxtexture.os.texture.as_ref()) else { return };
        let (width, height, mtl_tex) = (alloc.width, alloc.height, tex.as_id());
        unsafe {
            let descriptor = RcObjcId::from_owned(NonNull::new(msg_send![class!(MTLTextureDescriptor), new]).unwrap());
            let () = msg_send![descriptor.as_id(), setTextureType: MTLTextureType::D2];
            let () = msg_send![descriptor.as_id(), setDepth: 1u64];
            let () = msg_send![descriptor.as_id(), setStorageMode: MTLStorageMode::Shared];
            let () = msg_send![descriptor.as_id(), setUsage: MTLTextureUsage::ShaderRead];
            let () = msg_send![descriptor.as_id(), setWidth: width as u64];
            let () = msg_send![descriptor.as_id(), setHeight: height as u64];
            let () = msg_send![descriptor.as_id(), setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let Some(staging) = NonNull::new(msg_send![metal_cx.device, newTextureWithDescriptor: descriptor.as_id()]).map(RcObjcId::from_owned) else {
                crate::error!("pick: staging texture alloc failed");
                return;
            };
            let blit: ObjcId = msg_send![command_buffer, blitCommandEncoder];
            let () = msg_send![blit, copyFromTexture: mtl_tex toTexture: staging.as_id()];
            let () = msg_send![blit, synchronizeTexture: staging.as_id() slice: 0 level: 0];
            let () = msg_send![blit, endEncoding];
            let capture = Mutex::new(Some(staging));
            let sender = PICK_BUS.with(|(sender, _)| sender.clone());
            let () = msg_send![
                command_buffer,
                addCompletedHandler: &objc_block!(move |_cmd: ObjcId| {
                    if let Some(staging) = capture.lock().unwrap().take() {
                        let mut bytes = vec![0u8; width * height * 4];
                        let region = MTLRegion {
                            origin: MTLOrigin { x: 0, y: 0, z: 0 },
                            size: MTLSize { width: width as u64, height: height as u64, depth: 1 },
                        };
                        let _: () = msg_send![
                            staging.as_id(),
                            getBytes: bytes.as_mut_ptr()
                            bytesPerRow: width * 4
                            bytesPerImage: width * height * 4
                            fromRegion: region
                            mipmapLevel: 0
                            slice: 0
                        ];
                        let _ = sender.try_send((ticket, width, height, bytes));
                        crate::thread::wake_ui_loop();
                    }
                })
            ];
        }
    }
}
