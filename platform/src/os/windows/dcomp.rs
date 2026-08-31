//! DirectComposition presentation path for windows created with
//! `window.direct_composition`.
//!
//! Tree shape:
//!
//! ```text
//! root          (no bitmap; required so children can sit behind the UI)
//!  ├─ z < 0     (behind the UI; lower z further back. `DcompChildZ::BACKDROP`
//!  │             is i32::MIN so a fill stays under every other underlay)
//!  ├─ ui        (Makepad swap chain; z = 0, reserved)
//!  └─ z > 0     (in front of the UI; higher z closer to the viewer)
//! ```
//!
//! Default windows and popups keep using
//! `CreateSwapChainForHwnd` and never reach this module.

use std::collections::HashMap;

use crate::makepad_math::Vec4;
use crate::window::{DcompChildGeom, DcompChildId, DcompChildZ, DcompContent};
use crate::windows::core::{IUnknown, Interface};
use crate::windows::Win32::Foundation::HWND;
use crate::windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionRectangleClip,
    IDCompositionScaleTransform, IDCompositionTarget, IDCompositionVisual,
};
use crate::windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISwapChain1};

struct DcompChild {
    visual: IDCompositionVisual,
    clip: IDCompositionRectangleClip,
    scale: Option<IDCompositionScaleTransform>,
    z: DcompChildZ,
    /// What the caller asked for. Whether the visual is *actually* parented is
    /// `DcompWindow::stacked` — `AddVisual` can fail, and conflating the two
    /// loses the retry.
    shown: bool,
    pending_remove: bool,
    last_geom: Option<DcompChildGeom>,
}

/// Holds one window's composition tree. DirectComposition unbinds the tree from
/// the HWND as soon as the target is released, and a `WS_EX_NOREDIRECTIONBITMAP`
/// window without a tree draws nothing at all, so the COM objects must outlive
/// the window.
pub struct DcompWindow {
    device: IDCompositionDevice,
    hwnd: HWND,
    ui_swap_chain: IDXGISwapChain1,
    target: Option<IDCompositionTarget>,
    root: Option<IDCompositionVisual>,
    ui: Option<IDCompositionVisual>,
    ui_content_set: bool,
    ui_in_tree: bool,
    root_set: bool,
    children: HashMap<DcompChildId, DcompChild>,
    /// Colour last painted by [`Self::set_child_solid`], so a repeat call can
    /// skip building another swap chain. Any other content op invalidates it.
    solid: HashMap<DcompChildId, Vec4>,
    pending_content: HashMap<DcompChildId, Option<DcompContent>>,
    pending_geom: HashMap<DcompChildId, DcompChildGeom>,
    pending_create: HashMap<DcompChildId, DcompChildZ>,
    /// Children actually parented under `root`, in the order
    /// [`Self::restack_children`] added them. This is the source of truth for
    /// tree membership; `DcompChild::shown` is only the request.
    stacked: Vec<DcompChildId>,
    pending_commit: bool,
}

/// Probed before the HWND is created: `WS_EX_NOREDIRECTIONBITMAP` cannot be
/// removed after `CreateWindowExW`, so the caller has to learn that composition
/// is unavailable while it can still fall back to a redirection-bitmap window.
pub fn create_device(dxgi_device: &IDXGIDevice) -> Option<IDCompositionDevice> {
    match unsafe { DCompositionCreateDevice(dxgi_device) } {
        Ok(device) => Some(device),
        Err(error) => {
            crate::error!("DCompositionCreateDevice failed: {error}");
            None
        }
    }
}

/// Publishes `swap_chain` as the UI visual of `hwnd`, with a content-less root
/// so later child visuals can sit behind it.
///
/// Always returns a host: the HWND already has `WS_EX_NOREDIRECTIONBITMAP`, so
/// dropping the tree would leave the window blank forever. Incomplete bind
/// steps are retried from [`DcompWindow::commit_if_needed`].
pub fn bind_swapchain(
    device: IDCompositionDevice,
    hwnd: HWND,
    swap_chain: &IDXGISwapChain1,
) -> DcompWindow {
    let mut window = DcompWindow {
        device,
        hwnd,
        ui_swap_chain: swap_chain.clone(),
        target: None,
        root: None,
        ui: None,
        ui_content_set: false,
        ui_in_tree: false,
        root_set: false,
        children: HashMap::new(),
        solid: HashMap::new(),
        pending_content: HashMap::new(),
        pending_geom: HashMap::new(),
        pending_create: HashMap::new(),
        stacked: Vec::new(),
        pending_commit: true,
    };
    window.try_complete_bind();
    window
}

impl DcompContent {
    /// AddRef's `swap_chain` as `IUnknown` content for `SetContent`.
    pub fn from_swap_chain(swap_chain: &IDXGISwapChain1) -> Self {
        match swap_chain.cast::<IUnknown>() {
            Ok(unk) => unsafe { Self::from_raw_iunknown(unk.into_raw()) },
            Err(error) => {
                crate::error!("DcompContent::from_swap_chain: {error}");
                unsafe { Self::from_raw_iunknown(std::ptr::null_mut()) }
            }
        }
    }
}

impl DcompWindow {
    fn mark_dirty(&mut self) {
        self.pending_commit = true;
    }

    fn try_complete_bind(&mut self) {
        unsafe {
            if self.target.is_none() {
                match self.device.CreateTargetForHwnd(self.hwnd, true) {
                    Ok(target) => self.target = Some(target),
                    Err(error) => {
                        crate::error!(
                            "IDCompositionDevice::CreateTargetForHwnd failed: {error}; retrying"
                        );
                        self.mark_dirty();
                        return;
                    }
                }
            }
            if self.root.is_none() {
                match self.device.CreateVisual() {
                    Ok(root) => self.root = Some(root),
                    Err(error) => {
                        crate::error!(
                            "IDCompositionDevice::CreateVisual failed: {error}; retrying"
                        );
                        self.mark_dirty();
                        return;
                    }
                }
            }
            if self.ui.is_none() {
                match self.device.CreateVisual() {
                    Ok(ui) => self.ui = Some(ui),
                    Err(error) => {
                        crate::error!(
                            "IDCompositionDevice::CreateVisual (ui) failed: {error}; retrying"
                        );
                        self.mark_dirty();
                        return;
                    }
                }
            }
            let ui = self.ui.as_ref().unwrap();
            if !self.ui_content_set {
                if let Err(error) = ui.SetContent(&self.ui_swap_chain) {
                    crate::error!("IDCompositionVisual::SetContent failed: {error}; retrying");
                    self.mark_dirty();
                    return;
                }
                self.ui_content_set = true;
            }
            let root = self.root.as_ref().unwrap();
            if !self.ui_in_tree {
                if let Err(error) = root.AddVisual(ui, true, None::<&IDCompositionVisual>) {
                    crate::error!(
                        "IDCompositionVisual::AddVisual (ui) failed: {error}; retrying"
                    );
                    self.mark_dirty();
                    return;
                }
                self.ui_in_tree = true;
            }
            if !self.root_set {
                if let Err(error) = self.target.as_ref().unwrap().SetRoot(root) {
                    crate::error!("IDCompositionTarget::SetRoot failed: {error}; retrying");
                    self.mark_dirty();
                    return;
                }
                self.root_set = true;
                self.mark_dirty();
            }
        }
    }

    fn retry_pending_creates(&mut self) {
        let pending: Vec<(DcompChildId, DcompChildZ)> = self.pending_create.drain().collect();
        for (child_id, z) in pending {
            self.create_child(child_id, z);
        }
    }

    fn retry_pending_removes(&mut self) {
        let doomed: Vec<DcompChildId> = self
            .children
            .iter()
            .filter(|(_, child)| child.pending_remove)
            .map(|(id, _)| *id)
            .collect();
        for child_id in doomed {
            self.remove_child(child_id);
        }
    }

    /// Re-runs the stack for children that want to be shown but whose
    /// `AddVisual` has not landed yet (transient failure, or created before the
    /// root/ui visuals existed).
    fn retry_pending_attaches(&mut self) {
        let missing = self.children.iter().any(|(id, child)| {
            child.shown && !child.pending_remove && !self.stacked.contains(id)
        });
        if missing {
            self.restack_children();
        }
    }

    /// Rebinds the UI visual if the window replaced its swap chain (device
    /// loss). Costs one pointer compare on the settled path.
    ///
    /// `SetContent` is latched by `ui_content_set`, so without this a recreated
    /// chain would leave the UI visual holding a dead one — and a
    /// `WS_EX_NOREDIRECTIONBITMAP` window has no redirection bitmap to fall
    /// back to, so it would stay black for the rest of its life.
    pub fn sync_ui_swap_chain(&mut self, swap_chain: &IDXGISwapChain1) {
        if Interface::as_raw(&self.ui_swap_chain) == Interface::as_raw(swap_chain) {
            return;
        }
        self.ui_swap_chain = swap_chain.clone();
        self.ui_content_set = false;
        self.mark_dirty();
    }

    /// Runs deferred tree work and reports whether this window owes a `Commit`.
    ///
    /// The dirty flag is *not* cleared here: the composition device is shared by
    /// every composition window, so one `Commit` publishes all of them and the
    /// caller clears the flags only once it succeeded. Swap-chain pixels still
    /// reach DWM via Present; a Commit is only owed when visuals, offsets,
    /// clips, or content pointers change.
    pub fn prepare_commit(&mut self) -> bool {
        self.try_complete_bind();
        self.retry_pending_creates();
        self.retry_pending_removes();
        self.retry_pending_attaches();
        self.pending_commit && self.root_set
    }

    /// Called after a successful `Commit` on the shared device.
    pub fn commit_published(&mut self) {
        // A window still waiting on `SetRoot` had nothing published, so it keeps
        // its dirty flag and `try_complete_bind` retries on the next drain.
        if self.root_set {
            self.pending_commit = false;
        }
    }

    pub fn create_child(&mut self, child_id: DcompChildId, z: DcompChildZ) {
        if self.children.contains_key(&child_id) {
            crate::error!("DcompCreateChild: id {child_id:?} already exists");
            self.pending_create.remove(&child_id);
            return;
        }
        let z = z.sanitized();
        if !self.try_create_child(child_id, z) {
            self.pending_create.insert(child_id, z);
            self.mark_dirty();
            return;
        }
        if let Some(content) = self.pending_content.remove(&child_id) {
            self.set_child_content(child_id, content);
        }
        if let Some(geom) = self.pending_geom.remove(&child_id) {
            self.set_child_geom(child_id, geom);
        }
    }

    fn try_create_child(&mut self, child_id: DcompChildId, z: DcompChildZ) -> bool {
        unsafe {
            let visual = match self.device.CreateVisual() {
                Ok(visual) => visual,
                Err(error) => {
                    crate::error!(
                        "IDCompositionDevice::CreateVisual (child) failed: {error}; retrying"
                    );
                    return false;
                }
            };
            let clip = match self.device.CreateRectangleClip() {
                Ok(clip) => clip,
                Err(error) => {
                    crate::error!(
                        "IDCompositionDevice::CreateRectangleClip failed: {error}; retrying"
                    );
                    return false;
                }
            };
            if let Err(error) = visual.SetClip(&clip) {
                crate::error!("IDCompositionVisual::SetClip failed: {error}; retrying");
                return false;
            }
            if let Err(error) = clip.SetLeft2(0.0) {
                crate::error!("IDCompositionRectangleClip::SetLeft2 failed: {error}");
            }
            if let Err(error) = clip.SetTop2(0.0) {
                crate::error!("IDCompositionRectangleClip::SetTop2 failed: {error}");
            }
            self.children.insert(
                child_id,
                DcompChild {
                    visual,
                    clip,
                    scale: None,
                    z,
                    shown: false,
                    pending_remove: false,
                    last_geom: None,
                },
            );
            true
        }
    }

    pub fn set_child_content(&mut self, child_id: DcompChildId, content: Option<DcompContent>) {
        self.solid.remove(&child_id);
        self.apply_or_stash_content(child_id, content);
    }

    /// Whether `color` still has to be painted into `child_id`, so the caller
    /// can skip building a swap chain for a colour that is already there.
    pub fn solid_needs_paint(&self, child_id: DcompChildId, color: Vec4) -> bool {
        self.solid.get(&child_id) != Some(&color)
    }

    /// Like [`Self::set_child_content`], but remembers `color` so
    /// [`Self::solid_needs_paint`] can short-circuit the next identical call.
    /// Only records once the content is on a live visual: a redundant repaint is
    /// cheap, a skipped one leaves the child empty.
    pub fn set_child_solid(&mut self, child_id: DcompChildId, content: DcompContent, color: Vec4) {
        self.solid.remove(&child_id);
        if self.apply_or_stash_content(child_id, Some(content)) {
            self.solid.insert(child_id, color);
        }
    }

    /// Returns whether the content reached a live visual. `false` means it was
    /// stashed for a not-yet-created child, or `SetContent` failed.
    fn apply_or_stash_content(
        &mut self,
        child_id: DcompChildId,
        content: Option<DcompContent>,
    ) -> bool {
        let Some(child) = self.children.get_mut(&child_id) else {
            self.pending_content.insert(child_id, content);
            return false;
        };
        let ok = apply_content(&child.visual, content);
        if ok {
            self.mark_dirty();
        }
        ok
    }

    /// Moves an existing child to a new z. Restacking is what makes the result
    /// independent of the order children were created in.
    pub fn set_child_z(&mut self, child_id: DcompChildId, z: DcompChildZ) {
        let z = z.sanitized();
        // A child whose CreateVisual has not succeeded yet only exists as a
        // stashed z; updating that is enough, the retry picks it up.
        if let Some(pending) = self.pending_create.get_mut(&child_id) {
            *pending = z;
            return;
        }
        let shown = {
            let Some(child) = self.children.get_mut(&child_id) else {
                crate::error!("DcompSetChildZ: unknown id {child_id:?}");
                return;
            };
            if child.z == z {
                return;
            }
            child.z = z;
            child.shown
        };
        if shown {
            self.restack_children();
        }
    }

    pub fn set_child_geom(&mut self, child_id: DcompChildId, geom: DcompChildGeom) {
        let geom = geom.sanitized();
        if !self.children.contains_key(&child_id) {
            self.pending_geom.insert(child_id, geom);
            return;
        }
        let want_shown = geom.is_shown();
        let device = self.device.clone();
        let (wrote_geom, shown) = {
            let Some(child) = self.children.get_mut(&child_id) else {
                return;
            };
            if child
                .last_geom
                .is_some_and(|last| last.approx_eq(geom))
                && child.shown == want_shown
            {
                return;
            }
            let mut wrote_geom = true;
            unsafe {
                if let Err(error) = child.visual.SetOffsetX2(geom.x) {
                    crate::error!("IDCompositionVisual::SetOffsetX2 failed: {error}");
                    wrote_geom = false;
                }
                if let Err(error) = child.visual.SetOffsetY2(geom.y) {
                    crate::error!("IDCompositionVisual::SetOffsetY2 failed: {error}");
                    wrote_geom = false;
                }
                // Clip lives in pre-transform local space, so it must match the
                // swap-chain bitmap. Scale then stretches that bitmap to geom.
                let clip_w = geom.width / geom.scale_x;
                let clip_h = geom.height / geom.scale_y;
                if let Err(error) = child.clip.SetRight2(clip_w) {
                    crate::error!("IDCompositionRectangleClip::SetRight2 failed: {error}");
                    wrote_geom = false;
                }
                if let Err(error) = child.clip.SetBottom2(clip_h) {
                    crate::error!("IDCompositionRectangleClip::SetBottom2 failed: {error}");
                    wrote_geom = false;
                }
            }
            if !apply_scale(&device, child, geom.scale_x, geom.scale_y) {
                wrote_geom = false;
            }
            if wrote_geom {
                child.last_geom = Some(geom);
            }
            (wrote_geom, child.shown)
        };
        if wrote_geom {
            self.mark_dirty();
        }
        if want_shown != shown {
            if want_shown {
                self.attach_child(child_id);
            } else {
                self.detach_child(child_id);
            }
        }
    }

    pub fn remove_child(&mut self, child_id: DcompChildId) {
        self.solid.remove(&child_id);
        self.pending_content.remove(&child_id);
        self.pending_geom.remove(&child_id);
        self.pending_create.remove(&child_id);
        let Some(mut child) = self.children.remove(&child_id) else {
            return;
        };
        if self.stacked.contains(&child_id) {
            let removed = match self.root.as_ref() {
                Some(root) => match unsafe { root.RemoveVisual(&child.visual) } {
                    Ok(()) => true,
                    Err(error) => {
                        crate::error!("IDCompositionVisual::RemoveVisual failed: {error}");
                        false
                    }
                },
                // No root means nothing is parented; treat the entry as stale.
                None => true,
            };
            if !removed {
                child.pending_remove = true;
                child.shown = false;
                self.children.insert(child_id, child);
                self.mark_dirty();
                return;
            }
            self.stacked.retain(|id| *id != child_id);
        }
        let _ = unsafe { child.visual.SetContent(None::<&IUnknown>) };
        self.mark_dirty();
    }

    fn attach_child(&mut self, child_id: DcompChildId) {
        let Some(child) = self.children.get_mut(&child_id) else {
            return;
        };
        if child.pending_remove || child.shown {
            return;
        }
        child.shown = true;
        self.restack_children();
    }

    fn detach_child(&mut self, child_id: DcompChildId) {
        let Some(child) = self.children.get_mut(&child_id) else {
            return;
        };
        if !child.shown {
            return;
        }
        child.shown = false;
        self.restack_children();
    }

    /// Re-inserts every shown child relative to the UI visual so order follows
    /// z, not creation time. `AddVisual(..., false, ui)` on the most-negative
    /// first leaves it furthest back; each later behind-UI child is inserted
    /// immediately under `ui`, pushing the earlier ones down. Front children
    /// are inserted above `ui` highest-z first so they stay on top.
    ///
    /// Rebuilding the whole stack (rather than splicing one child in) keeps the
    /// order correct no matter what created or hid what, and no intermediate
    /// state is observable: DWM only sees the tree at the next `Commit`.
    fn restack_children(&mut self) {
        let (Some(root), Some(ui)) = (self.root.clone(), self.ui.clone()) else {
            // Nothing is parented yet; `retry_pending_attaches` re-runs this
            // once the bind completes.
            self.mark_dirty();
            return;
        };
        let mut touched = !self.stacked.is_empty();
        for id in std::mem::take(&mut self.stacked) {
            if let Some(child) = self.children.get(&id) {
                let _ = unsafe { root.RemoveVisual(&child.visual) };
            }
        }
        let candidates: Vec<(DcompChildZ, DcompChildId)> = self
            .children
            .iter()
            .filter(|(_, child)| child.shown && !child.pending_remove)
            .map(|(id, child)| (child.z, *id))
            .collect();
        let (behind, front) = stacking_order(candidates);
        for id in behind {
            let Some(child) = self.children.get(&id) else {
                continue;
            };
            if let Err(error) = unsafe { root.AddVisual(&child.visual, false, Some(&ui)) } {
                crate::error!("IDCompositionVisual::AddVisual (behind) failed: {error}");
                continue;
            }
            self.stacked.push(id);
            touched = true;
        }
        for id in front {
            let Some(child) = self.children.get(&id) else {
                continue;
            };
            if let Err(error) = unsafe { root.AddVisual(&child.visual, true, Some(&ui)) } {
                crate::error!("IDCompositionVisual::AddVisual (front) failed: {error}");
                continue;
            }
            self.stacked.push(id);
            touched = true;
        }
        if touched {
            self.mark_dirty();
        }
    }
}

/// Splits shown children into the order [`DcompWindow::restack_children`] must
/// insert them: first the ones going below `ui` (most negative z first, so it
/// ends up furthest back), then the ones going above `ui` (highest z first, so
/// it ends up on top). Ties on z fall back to child id, i.e. creation order.
///
/// Pure, so the z-ordering is testable without a DirectComposition device.
/// Dispatch is on [`DcompChildZ::is_front`] rather than a `z > 0` / `z < 0`
/// pair, so an unsanitized `z == 0` still lands in exactly one of the two.
fn stacking_order(
    mut candidates: Vec<(DcompChildZ, DcompChildId)>,
) -> (Vec<DcompChildId>, Vec<DcompChildId>) {
    candidates.sort_unstable_by_key(|(z, id)| (z.sanitized().0, id.0));
    let (front, behind): (Vec<_>, Vec<_>) =
        candidates.into_iter().partition(|(z, _)| z.sanitized().is_front());
    (
        behind.into_iter().map(|(_, id)| id).collect(),
        front.into_iter().rev().map(|(_, id)| id).collect(),
    )
}

fn apply_scale(
    device: &IDCompositionDevice,
    child: &mut DcompChild,
    scale_x: f32,
    scale_y: f32,
) -> bool {
    unsafe {
        if child.scale.is_none() {
            match create_scale_transform(device) {
                Ok(scale) => {
                    if let Err(error) = visual_set_transform(&child.visual, &scale) {
                        crate::error!("IDCompositionVisual::SetTransform failed: {error}");
                        return false;
                    }
                    child.scale = Some(scale);
                }
                Err(error) => {
                    crate::error!("IDCompositionDevice::CreateScaleTransform failed: {error}");
                    return false;
                }
            }
        }
        let Some(scale) = child.scale.as_ref() else {
            return false;
        };
        if let Err(error) = scale_set_xy(scale, scale_x, scale_y) {
            crate::error!("IDCompositionScaleTransform::SetScale failed: {error}");
            return false;
        }
        true
    }
}

unsafe fn create_scale_transform(
    device: &IDCompositionDevice,
) -> crate::windows::core::Result<IDCompositionScaleTransform> {
    let mut raw = std::ptr::null_mut();
    unsafe {
        (Interface::vtable(device).CreateScaleTransform)(Interface::as_raw(device), &mut raw)
    }
    .and_then(|| unsafe { crate::windows::core::Type::from_abi(raw) })
}

unsafe fn visual_set_transform(
    visual: &IDCompositionVisual,
    transform: &IDCompositionScaleTransform,
) -> crate::windows::core::Result<()> {
    unsafe {
        (Interface::vtable(visual).SetTransform)(
            Interface::as_raw(visual),
            Interface::as_raw(transform),
        )
    }
    .ok()
}

unsafe fn scale_set_xy(
    scale: &IDCompositionScaleTransform,
    scale_x: f32,
    scale_y: f32,
) -> crate::windows::core::Result<()> {
    unsafe {
        (Interface::vtable(scale).SetScaleX2)(Interface::as_raw(scale), scale_x).ok()?;
        (Interface::vtable(scale).SetScaleY2)(Interface::as_raw(scale), scale_y).ok()
    }
}

fn apply_content(visual: &IDCompositionVisual, content: Option<DcompContent>) -> bool {
    unsafe {
        match content {
            Some(content) => {
                let ptr = content.into_raw();
                if ptr.is_null() {
                    if let Err(error) = visual.SetContent(None::<&IUnknown>) {
                        crate::error!("IDCompositionVisual::SetContent(None) failed: {error}");
                        return false;
                    }
                    return true;
                }
                let unk = IUnknown::from_raw(ptr);
                if let Err(error) = visual.SetContent(&unk) {
                    crate::error!("IDCompositionVisual::SetContent failed: {error}");
                    return false;
                }
                true
            }
            None => {
                if let Err(error) = visual.SetContent(None::<&IUnknown>) {
                    crate::error!("IDCompositionVisual::SetContent(None) failed: {error}");
                    return false;
                }
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ids: &[DcompChildId]) -> Vec<u64> {
        ids.iter().map(|id| id.0).collect()
    }

    #[test]
    fn behind_children_are_inserted_from_the_back_forward() {
        // AddVisual(.., false, ui) puts each one directly under `ui`, so the
        // first inserted ends up furthest back: BACKDROP must come first even
        // though it was created last.
        let (behind, front) = stacking_order(vec![
            (DcompChildZ::BEHIND, DcompChildId(1)),
            (DcompChildZ::BACKDROP, DcompChildId(2)),
        ]);
        assert_eq!(ids(&behind), vec![2, 1]);
        assert!(front.is_empty());
    }

    #[test]
    fn front_children_are_inserted_from_the_top_down() {
        // AddVisual(.., true, ui) puts each one directly above `ui`, so the
        // highest z has to go in first to stay on top.
        let (behind, front) = stacking_order(vec![
            (DcompChildZ(1), DcompChildId(1)),
            (DcompChildZ(9), DcompChildId(2)),
        ]);
        assert!(behind.is_empty());
        assert_eq!(ids(&front), vec![2, 1]);
    }

    #[test]
    fn equal_z_children_stack_in_creation_order() {
        let (behind, _) = stacking_order(vec![
            (DcompChildZ::BEHIND, DcompChildId(7)),
            (DcompChildZ::BEHIND, DcompChildId(3)),
        ]);
        // Lower id inserted first, so the later-created child sits in front.
        assert_eq!(ids(&behind), vec![3, 7]);
    }

    #[test]
    fn an_unsanitized_ui_z_still_gets_stacked() {
        // `create_child` sanitizes, but a child must never fall out of both
        // passes and end up permanently invisible if that ever changes.
        let (behind, front) = stacking_order(vec![(DcompChildZ::UI, DcompChildId(1))]);
        assert_eq!(ids(&behind), vec![1]);
        assert!(front.is_empty());
    }
}
