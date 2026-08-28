use {
    crate::{
        cx_draw::CxDraw,
        draw_list_2d::DrawList2d,
        makepad_math::{Rect, Vec2Index, Vec2d},
        makepad_platform::{DrawListId, DrawPassId, LiveId},
        makepad_script::ScriptNew,
        turtle::{AlignEntry, FinishedWalk, Turtle, Walk},
    },
    std::{ops::Deref, ops::DerefMut},
};

pub struct Cx2d<'a, 'b> {
    pub cx: &'b mut CxDraw<'a>,
    pub(crate) overlay_id: Option<DrawListId>,
    pub(crate) overlay_pass_id: Option<DrawPassId>,
    pub(crate) overlay_draw_depth: usize,
    /// Increments once per overlay sub-list begun this frame; stamped onto the
    /// sub-list so the Overlay can composite in draw order.
    pub(crate) overlay_seq: u64,

    //pub (crate) overlay_sweep_lock: Option<Rc<RefCell<Area>>>,
    pub(crate) turtles: Vec<Turtle>,
    pub(crate) finished_rows: Vec<usize>,
    pub(crate) finished_walks: Vec<FinishedWalk>,
    pub(crate) turtle_clips: Vec<(Vec2d, Vec2d)>,
    pub(crate) align_list: Vec<AlignEntry>,
    pub(crate) draw_call_parent_stack: Vec<u64>,
    pub(crate) draw_call_parent_next: u64,
    /// The wireframe used by the exploded z-layer view to give every turtle
    /// scope a visible frame. Built on first use so an app that never opens
    /// the mode never constructs it.
    pub(crate) sploded_hairline: Option<Box<crate::shader::draw_sploded_hairline::DrawSplodedHairline>>,
}

impl<'a, 'b> Deref for Cx2d<'a, 'b> {
    type Target = CxDraw<'a>;
    fn deref(&self) -> &Self::Target {
        self.cx
    }
}
impl<'a, 'b> DerefMut for Cx2d<'a, 'b> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.cx
    }
}

impl<'a, 'b> Cx2d<'a, 'b> {
    pub fn new(cx: &'b mut CxDraw<'a>) -> Self {
        let mut draw_call_parent_stack = Vec::with_capacity(256);
        // Root scope id.
        draw_call_parent_stack.push(1);
        Self {
            overlay_id: None,
            overlay_pass_id: None,
            overlay_draw_depth: 0,
            overlay_seq: 0,
            cx,
            turtle_clips: Vec::with_capacity(1024),
            finished_rows: Vec::with_capacity(1024),
            finished_walks: Vec::with_capacity(1024),
            turtles: Vec::with_capacity(64),
            align_list: Vec::with_capacity(4096),
            draw_call_parent_stack,
            draw_call_parent_next: 2,
            sploded_hairline: None,
        }
    }

    pub fn is_drawing_overlay(&self) -> bool {
        self.overlay_draw_depth > 0
    }

    /// Draw one turtle scope's wireframe frame, while — and only while — the
    /// exploded z-layer view is up.
    ///
    /// A parent whose children fill it completely has no pixels of its own in
    /// flat 2D; in the exploded view its plane would be an invisible gap. This
    /// gives every nesting level a frame to see, and to click.
    ///
    /// Costs nothing when the mode is off: one bool test, and the wireframe is
    /// never even constructed.
    pub(crate) fn draw_sploded_hairline(&mut self, rect: Rect) {
        if !self.cx.sploded_hairlines_active() {
            return;
        }
        if rect.size.x < 1.0 || rect.size.y < 1.0 {
            return;
        }
        if self.draw_list_stack.is_empty() {
            return;
        }
        if self.sploded_hairline.is_none() {
            // `script_new_with_default` — not `script_new` — because the
            // registered type default is where the shader binding lives.
            let hairline = self.cx.with_vm(|vm| {
                crate::shader::draw_sploded_hairline::DrawSplodedHairline::script_new_with_default(
                    vm,
                )
            });
            if hairline.draw_vars.draw_shader_id.is_none() {
                crate::makepad_platform::error!(
                    "sploded hairline: shader did not bind; scope frames disabled"
                );
            }
            self.sploded_hairline = Some(Box::new(hairline));
        }
        let level = self.cx.nesting_depth as f32;
        let mut hairline = self.sploded_hairline.take().unwrap();
        hairline.draw_scope(self, rect, level);
        self.sploded_hairline = Some(hairline);
    }

    #[inline]
    pub fn push_draw_call_parent(&mut self) {
        let id = self.draw_call_parent_next;
        self.draw_call_parent_next = self.draw_call_parent_next.wrapping_add(1).max(2);
        self.draw_call_parent_stack.push(id);
    }

    #[inline]
    pub fn pop_draw_call_parent(&mut self) {
        if self.draw_call_parent_stack.len() > 1 {
            self.draw_call_parent_stack.pop();
        }
    }

    #[inline]
    fn pack_draw_call_group(base: u64, lane: u8) -> LiveId {
        LiveId((base << 8) | lane as u64)
    }

    #[inline]
    pub fn draw_call_group_parent(&self) -> LiveId {
        let len = self.draw_call_parent_stack.len();
        let id = if len >= 2 {
            self.draw_call_parent_stack[len - 2]
        } else {
            self.draw_call_parent_stack[0]
        };
        LiveId(id)
    }

    #[inline]
    pub fn draw_call_group_current(&self) -> LiveId {
        LiveId(*self.draw_call_parent_stack.last().unwrap_or(&1))
    }

    #[inline]
    pub fn draw_call_group_background(&self) -> LiveId {
        let len = self.draw_call_parent_stack.len();
        let base = if len >= 2 {
            self.draw_call_parent_stack[len - 2]
        } else {
            self.draw_call_parent_stack[0]
        };
        Self::pack_draw_call_group(base, 0)
    }

    #[inline]
    pub fn draw_call_group_content(&self) -> LiveId {
        let len = self.draw_call_parent_stack.len();
        if len >= 2 {
            // Content of child scopes share a common lane under their parent.
            let base = self.draw_call_parent_stack[len - 2];
            return Self::pack_draw_call_group(base, 1);
        }
        // Top-level content stays in lane 0 so it does not merge with child-content lane 1.
        Self::pack_draw_call_group(self.draw_call_parent_stack[0], 0)
    }

    pub fn will_redraw(&self, draw_list_2d: &mut DrawList2d, walk: Walk) -> bool {
        // ok so we need to check if our turtle position has changed since last time.
        // if it did, we redraw
        let rect = self.peek_walk_turtle(walk);
        if draw_list_2d.dirty_check_rect != rect {
            draw_list_2d.dirty_check_rect = rect;
            return true;
        }
        self.draw_event
            .draw_list_will_redraw(self, draw_list_2d.draw_list.id())
    }

    pub fn will_redraw_check_axis(
        &self,
        draw_list_2d: &mut DrawList2d,
        size: f64,
        axis: Vec2Index,
    ) -> bool {
        // ok so we need to check if our turtle position has changed since last time.
        // if it did, we redraw
        if draw_list_2d.dirty_check_rect.size.index(axis) != size {
            draw_list_2d.dirty_check_rect.size.set_index(axis, size);
            return true;
        }
        self.draw_event
            .draw_list_will_redraw(self, draw_list_2d.draw_list.id())
    }
}
