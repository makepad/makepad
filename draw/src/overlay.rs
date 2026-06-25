use {
    /*std::{
        rc::Rc,
        cell::{RefCell},
    },*/
    crate::{cx_2d::Cx2d, makepad_platform::*},
};

#[derive(Debug, Script, ScriptHook)]
pub struct Overlay {
    // draw info per UI element
    #[new]
    pub draw_list: DrawList,
    //pub (crate) sweep_lock: Rc<RefCell<Area>>,
}

impl Overlay {
    pub fn handle_event(&self, _cx: &Cx, _event: &Event) {
        /*let area = self.sweep_lock.borrow().clone();
        if !area.is_empty(){
            match event{
                Event::MouseMove(e)=>e.sweep_lock.set(area),
                Event::MouseDown(e)=>e.sweep_lock.set(area),
                Event::Scroll(e)=>e.sweep_lock.set(area),
                _=>()
            }
        }*/
    }

    pub fn begin(&self, cx: &mut Cx2d) {
        // mark our overlay_id on cx
        cx.overlay_id = Some(self.draw_list.id());
        cx.overlay_pass_id = None;
        // cx.overlay_sweep_lock = Some(self.sweep_lock.clone());
    }

    pub fn begin_for_pass(&self, cx: &mut Cx2d, pass_id: DrawPassId) {
        // mark our overlay_id on cx
        cx.overlay_id = Some(self.draw_list.id());
        cx.overlay_pass_id = Some(pass_id);
        // cx.overlay_sweep_lock = Some(self.sweep_lock.clone());
    }

    pub fn end(&self, cx: &mut Cx2d) {
        cx.overlay_id = None;
        cx.overlay_pass_id = None;
        let parent_id = cx.draw_list_stack.last().cloned().unwrap();
        let redraw_id = cx.redraw_id;
        cx.draw_lists[parent_id].append_sub_list(redraw_id, self.draw_list.id());

        // flush out all overlays that have a different redraw id than their parent
        // this means it didn't
        for i in 0..cx.draw_lists[self.draw_list.id()].draw_items.len() {
            if let Some(sub_id) = cx.draw_lists[self.draw_list.id()].draw_items[i].sub_list() {
                // If the sub draw list's owning widget was DROPPED (its slot is in the free pool),
                // its overlay must be removed — otherwise a glass widget whose chat message was
                // cleared/recycled stays stuck (it's never drawn again to clear itself, and the
                // redraw_id heuristic below can't catch a freed-but-not-yet-reused slot). Only
                // freed lists hit this; live (still-drawn) glass keeps its slot, so no flicker.
                if cx.draw_lists.is_id_freed(sub_id) {
                    cx.draw_lists[self.draw_list.id()].clear_sub_list(sub_id);
                    continue;
                }
                // Use checked_index to safely access draw lists that might have been recycled
                if let Some(sub_draw_list) = cx.draw_lists.checked_index(sub_id) {
                    if let Some(cfp) = sub_draw_list.codeflow_parent_id {
                        // Also check the parent draw list safely
                        if let Some(parent_draw_list) = cx.draw_lists.checked_index(cfp) {
                            if parent_draw_list.redraw_id != sub_draw_list.redraw_id {
                                cx.draw_lists[self.draw_list.id()].clear_sub_list(sub_id);
                            }
                        } else {
                            // Parent draw list was recycled, clear this sub list
                            cx.draw_lists[self.draw_list.id()].clear_sub_list(sub_id);
                        }
                    }
                } else {
                    // Sub draw list was recycled, clear it from our list
                    cx.draw_lists[self.draw_list.id()].clear_sub_list(sub_id);
                }
            }
        }
    }
}
