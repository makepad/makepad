//! A window's badge (`window_kind::BADGE`): a small transparent `CAMetalLayer` over a window's
//! content, a sublayer of the window's own layer. Core Animation composites it over whatever the
//! window shows (a lent window's app frames too), so the app's frames neither wait for it nor
//! pay for it; a layer takes no input (hit testing is the view's), so the app keeps every click.
//! Session 0 draws it like any window (its frames present its drawables). Main thread only.

use super::window::Win;
use super::Shared;
use crate::types::*;
use makepad_apple_sys::*;

pub struct Badge {
    pub sid: u32,
    pub id: Id,
    /// the window it sits on (session 0's; it may be lent)
    pub parent: &'static Win,
    layer: ObjcId,
}

impl Badge {
    /// Makes the badge on `parent` at `geom` (points in the parent's content, top left).
    pub fn create(sid: u32, id: Id, parent: &'static Win, geom: [f32; 4]) -> Badge {
        let shared = Shared::global();
        let layer: ObjcId = unsafe { msg_send![class!(CAMetalLayer), new] };
        unsafe {
            let () = msg_send![layer, setDevice: shared.device()];
            let () = msg_send![layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            // premultiplied pixels over the window's content
            let () = msg_send![layer, setOpaque: NO];
            let () = msg_send![layer, setPresentsWithTransaction: NO];
            let () = msg_send![layer, setMaximumDrawableCount: 3u64];
            let () = msg_send![layer, setDisplaySyncEnabled: YES];
            let () = msg_send![layer, setAllowsNextDrawableTimeout: YES];
            // a snapshot of its frame copies the drawable
            let () = msg_send![layer, setFramebufferOnly: NO];
            let () = msg_send![layer, setHidden: NO];
        }
        let badge = Badge { sid, id, parent, layer };
        badge.set_geom(geom);
        unsafe {
            let () = msg_send![class!(CATransaction), begin];
            let () = msg_send![class!(CATransaction), setDisableActions: YES];
            let () = msg_send![parent.layer, addSublayer: layer];
            let () = msg_send![class!(CATransaction), commit];
        }
        shared.register_layer(sid, id, RcObjcId::from_unowned(std::ptr::NonNull::new(layer).expect("badge layer")));
        badge
    }

    /// Places the badge at `geom` (points in the parent's content, top left), at the parent's
    /// backing scale; it keeps its distance to the parent's bottom-right corner while the parent
    /// resizes (session 0 places it again after).
    pub fn set_geom(&self, geom: [f32; 4]) {
        let [x, y, w, h] = geom.map(|v| if v.is_finite() { v as f64 } else { 0.0 });
        unsafe {
            let bounds: NSRect = msg_send![self.parent.layer, bounds];
            let flipped: bool = msg_send![self.parent.layer, isGeometryFlipped];
            let top = if flipped { y } else { bounds.size.height - y - h };
            let scale = self.parent.dpi();
            let () = msg_send![class!(CATransaction), begin];
            let () = msg_send![class!(CATransaction), setDisableActions: YES];
            let () = msg_send![self.layer, setFrame: NSRect { origin: NSPoint { x, y: top }, size: NSSize { width: w.max(1.0), height: h.max(1.0) } }];
            // kCALayerMinXMargin | (the margin away from the bottom edge)
            let away_from_bottom: u64 = if flipped { 1 << 3 } else { 1 << 5 };
            let () = msg_send![self.layer, setAutoresizingMask: (1u64 << 0) | away_from_bottom];
            let () = msg_send![self.layer, setContentsScale: scale];
            let () = msg_send![self.layer, setDrawableSize: CGSize { width: (w * scale).round().max(1.0), height: (h * scale).round().max(1.0) }];
            let () = msg_send![class!(CATransaction), commit];
        }
    }

    /// Takes the badge off its window.
    pub fn remove(self) {
        Shared::global().unregister_layer(self.sid, self.id);
        unsafe {
            let () = msg_send![class!(CATransaction), begin];
            let () = msg_send![class!(CATransaction), setDisableActions: YES];
            let () = msg_send![self.layer, removeFromSuperlayer];
            let () = msg_send![class!(CATransaction), commit];
            let () = msg_send![self.layer, release];
        }
    }
}
