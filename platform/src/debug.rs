use std::cell::RefCell;
use std::rc::Rc;
use crate::makepad_math::{Rect, Vec4, DVec2};
use crate::cx::Cx;
use crate::makepad_error_log::*;
use crate::draw_list::DrawListId;
use std::fmt::Write;

#[derive(Clone, Default)]
pub struct DebugInner {
    pub rects: Vec<(Rect, Vec4)>,
    pub points: Vec<(DVec2, Vec4)>,
    pub labels: Vec<(DVec2, Vec4, String)>,
}

#[derive(Clone, Default)]
pub struct Debug(Rc<RefCell<DebugInner >>);

impl Debug {
    const R: Vec4 = Vec4 {x: 1.0, y: 0.0, z: 0.0, w: 1.0};
    const G: Vec4 = Vec4 {x: 0.0, y: 1.0, z: 0.0, w: 1.0};
    const B: Vec4 = Vec4 {x: 0.0, y: 0.0, z: 1.0, w: 1.0};
    
    pub fn point(&self, color: Vec4, p: DVec2) {
        let mut inner = self.0.borrow_mut();
        inner.points.push((p, color));
    }
    
    pub fn point_r(&self, p: DVec2) {self.point(Self::R, p)}
    pub fn point_g(&self, p: DVec2) {self.point(Self::G, p)}
    pub fn point_b(&self, p: DVec2) {self.point(Self::B, p)}

    pub fn label(&self, color: Vec4, p: DVec2, label:String) {
        let mut inner = self.0.borrow_mut();
        inner.labels.push((p, color,label));
    }
    
    pub fn label_r(&self, p: DVec2, label:String) {self.label(Self::R, p, label)}
    pub fn label_g(&self, p: DVec2, label:String) {self.label(Self::G, p, label)}
    pub fn label_b(&self, p: DVec2, label:String) {self.label(Self::B, p, label)}

    pub fn rect(&self, color: Vec4, p: Rect) {
        let mut inner = self.0.borrow_mut();
        inner.rects.push((p, color));
    }
    pub fn rect_r(&self, r: Rect) {self.rect(Self::R, r)}
    pub fn rect_g(&self, r: Rect) {self.rect(Self::G, r)}
    pub fn rect_b(&self, r: Rect) {self.rect(Self::B, r)}
    
    pub fn has_data(&self)->bool{
        let inner = self.0.borrow();
        !inner.points.is_empty() || !inner.rects.is_empty() || !inner.labels.is_empty()
    }
    
    pub fn take_rects(&self)->Vec<(Rect, Vec4)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.rects);
        swap
    }

    pub fn take_points(&self)->Vec<(DVec2, Vec4)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.points);
        swap
    }

    pub fn take_labels(&self)->Vec<(DVec2, Vec4, String)>{
        let mut inner = self.0.borrow_mut();
        let mut swap = Vec::new();
        std::mem::swap(&mut swap, &mut inner.labels);
        swap
    }
}

impl Cx{
    
    pub fn debug_draw_tree(&self, dump_instances: bool, draw_list_id: DrawListId) {
        fn debug_draw_tree_recur(cx: &Cx, dump_instances: bool, s: &mut String, draw_list_id: DrawListId, depth: usize) {
            //if draw_list_id >= cx.draw_lists.len() {
            //    writeln!(s, "---------- Drawlist still empty ---------").unwrap();
            //    return
            //}
            let mut indent = String::new();
            for _i in 0..depth {
                indent.push_str("|   ");
            }
            let draw_items_len = cx.draw_lists[draw_list_id].draw_items.len();
            //if draw_list_id == 0 {
            //    writeln!(s, "---------- Begin Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            // }
            //let rect = cx.draw_lists[draw_list_id].rect;
            //let scroll = cx.draw_lists[draw_list_id].get_local_scroll();
            writeln!(
                s,
                "{}{} {:?}: len:{}",
                indent,
                cx.draw_lists[draw_list_id].debug_id,
                draw_list_id,
                draw_items_len,
            ).unwrap();
            indent.push_str("  ");
            let mut indent = String::new();
            for _i in 0..depth + 1 {
                indent.push_str("|   ");
            }
            for draw_item_id in 0..draw_items_len {
                if let Some(sub_list_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id].sub_list() {
                    debug_draw_tree_recur(cx, dump_instances, s, sub_list_id, depth + 1);
                }
                else {
                    let cxview = &cx.draw_lists[draw_list_id];
                    let darw_item = &cxview.draw_items[draw_item_id];
                    let draw_call = darw_item.draw_call().unwrap();
                    let sh = &cx.draw_shaders.shaders[draw_call.draw_shader.draw_shader_id];
                    let slots = sh.mapping.instances.total_slots;
                    let instances = darw_item.instances.as_ref().unwrap().len() / slots;
                    writeln!(
                        s,
                        "{}{}({}) sid:{} inst:{}",
                        indent,
                        draw_call.options.debug_id.unwrap_or(sh.class_prop),
                        sh.type_name,
                        draw_call.draw_shader.draw_shader_id,
                        instances,
                    ).unwrap();
                    // lets dump the instance geometry
                    if dump_instances {
                        for inst in 0..instances.min(1) {
                            let mut out = String::new();
                            let mut off = 0;
                            for input in &sh.mapping.instances.inputs {
                                let buf = darw_item.instances.as_ref().unwrap();
                                match input.slots {
                                    1 => out.push_str(&format!("{}:{} ", input.id, buf[inst * slots + off])),
                                    2 => out.push_str(&format!("{}:v2({},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off])),
                                    3 => out.push_str(&format!("{}:v3({},{},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off], buf[inst * slots + 1 + off])),
                                    4 => out.push_str(&format!("{}:v4({},{},{},{}) ", input.id, buf[inst * slots + off], buf[inst * slots + 1 + off], buf[inst * slots + 2 + off], buf[inst * slots + 3 + off])),
                                    _ => {}
                                }
                                off += input.slots;
                            }
                            writeln!(s, "  {}instance {}: {}", indent, inst, out).unwrap();
                        }
                    }
                }
            }
            //if draw_list_id == 0 {
            //    writeln!(s, "---------- End Debug draw tree for redraw_id: {} ---------", cx.redraw_id).unwrap();
            //}
        }
        
        let mut s = String::new();
        debug_draw_tree_recur(self, dump_instances, &mut s, draw_list_id, 0);
        log!("{}", s);
    }

}
