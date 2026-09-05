use {
    crate::{
        animator::*, makepad_derive_widget::*, makepad_draw::*, scroll_bars::ScrollBars,
        scroll_shadow::DrawScrollShadow, widget::*, widget_tree::CxWidgetExt,
    },
    std::collections::HashSet,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // Register draw shaders
    set_type_default() do #(DrawBgQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    set_type_default() do #(DrawNameText::script_shader(vm)){
        ..mod.draw.DrawText
    }

    let GitStatusDotKind = set_type_default() do #(GitStatusDotKind::script_api(vm))
    mod.widgets.GitStatusDotKind = GitStatusDotKind

    set_type_default() do #(DrawIconQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: instance(#fff)
        color_active: instance(#fff)
        pixel: fn() {
            let icon_color = mix(
                self.color * self.scale,
                self.color_active,
                self.active
            )
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let w = self.rect_size.x
            let h = self.rect_size.y

            if self.is_folder > 0.5 {
                sdf.box(0.02 * w, 0.36 * h, 0.86 * w, 0.40 * h, 0.75)
                sdf.box(0.02 * w, 0.28 * h, 0.50 * w, 0.30 * h, 1.0)
                sdf.union()
                sdf.fill(icon_color)
            }
            return sdf.result
        }
    }

    set_type_default() do #(DrawStatusDotQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        status_kind: instance(GitStatusDotKind.None)
        color_new: #x58c26d
        color_modified: #FA0
        color_deleted: #xd86464
        color_mixed: #xd86464
        pixel: fn() {
            let dot_color = match self.status_kind {
                GitStatusDotKind.New => self.color_new
                GitStatusDotKind.Modified => self.color_modified
                GitStatusDotKind.Deleted => self.color_deleted
                GitStatusDotKind.Mixed => self.color_mixed
                _ => self.color_mixed
            }
            let dot_blend = match self.status_kind {
                GitStatusDotKind.New => 1f
                GitStatusDotKind.Modified => 1f
                GitStatusDotKind.Mixed => 1f
                _ => 0f
            }
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.circle(
                0.5 * self.rect_size.x,
                0.5 * self.rect_size.y,
                min(self.rect_size.x, self.rect_size.y) * 0.34
            )
            return dot_color * sdf.fill(vec4(1f, 1f, 1f, dot_blend)).w
        }
    }

    // Register FileTreeNode as a widget: every row is a real widget with a
    // uid, so the design tweaker can pick it and style its template.
    mod.widgets.FileTreeNodeBase = #(FileTreeNode::register_widget(vm))
    mod.widgets.FileTreeBase = #(FileTree::register_widget(vm))

    mod.widgets.FileTreeNode = set_type_default() do mod.widgets.FileTreeNodeBase{
        align: Align{y: 0.5}
        padding: Inset{left: theme.space_2}
        is_folder: false
        indent_width: theme.space_2
        min_drag_distance: 10.0

        draw_bg +: {
            color_1: instance(theme.color_bg_even)
            color_2: instance(theme.color_bg_odd)
            color_active: instance(theme.color_highlight)

            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    0.,
                    (-2.),
                    self.rect_size.x,
                    self.rect_size.y + 3.0,
                    1.
                )
                sdf.fill_keep(
                    mix(
                        mix(
                            self.color_1,
                            self.color_2,
                            self.is_even
                        ),
                        self.color_active,
                        self.active
                    )
                )
                return sdf.result
            }
        }

        draw_icon +: {
            color: instance(theme.color_label_inner)
            color_active: instance(theme.color_label_inner_active)
        }

        draw_text +: {
            color: theme.color_label_inner
            color_active: theme.color_label_inner_active

            get_color: fn() {
                return mix(
                    self.color * self.scale,
                    self.color_active,
                    self.active
                )
            }

            text_style: theme.font_regular{
                font_size: theme.font_size_p
            }
        }

        icon_walk: Walk{
            width: (theme.data_icon_width - 2.0)
            height: theme.data_icon_height
            margin: Inset{right: theme.space_1}
        }

        status_dot_walk: Walk{
            width: 6.0
            height: 6.0
            margin: Inset{right: theme.space_1}
        }

        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Forward {duration: 0.2}}
                    apply: {
                        hover: 0.0
                        draw_bg: {hover: 0.0}
                        draw_text: {hover: 0.0}
                        draw_icon: {hover: 0.0}
                    }
                }

                on: AnimatorState{
                    cursor: MouseCursor.Hand
                    from: {all: Play.Snap}
                    apply: {
                        hover: 1.0
                        draw_bg: {hover: 1.0}
                        draw_text: {hover: 1.0}
                        draw_icon: {hover: 1.0}
                    }
                }
            }

            focus: {
                default: @on
                on: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {focussed: 1.0}
                }

                off: AnimatorState{
                    from: {all: Play.Forward {duration: 0.1}}
                    apply: {focussed: 0.0}
                }
            }

            select: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Forward {duration: 0.1}}
                    apply: {
                        active: 0.0
                        draw_bg: {active: 0.0}
                        draw_text: {active: 0.0}
                        draw_icon: {active: 0.0}
                    }
                }
                on: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {
                        active: 1.0
                        draw_bg: {active: 1.0}
                        draw_text: {active: 1.0}
                        draw_icon: {active: 1.0}
                    }
                }
            }

            open: {
                default: @off
                off: AnimatorState{
                    redraw: true
                    from: {all: Play.Forward {duration: 0.2}}
                    ease: Ease.ExpDecay {d1: 0.80 d2: 0.97}
                    apply: {
                        opened: 0.0
                        draw_bg: {opened: 0.0}
                        draw_text: {opened: 0.0}
                        draw_icon: {opened: 0.0}
                    }
                }

                on: AnimatorState{
                    from: {all: Play.Forward {duration: 0.2}}
                    ease: Ease.ExpDecay {d1: 0.82 d2: 0.95}
                    redraw: true
                    apply: {
                        opened: 1.0
                        draw_bg: {opened: 1.0}
                        draw_text: {opened: 1.0}
                        draw_icon: {opened: 1.0}
                    }
                }
            }
        }
    }

    mod.widgets.FileTree = mod.widgets.FileTreeBase{
        flow: Down

        scroll_bars: mod.widgets.ScrollBars{}
        node_height: theme.data_item_height
        clip_x: true
        clip_y: true

        file_node: mod.widgets.FileTreeNode{
            is_folder: false
            draw_bg +: {is_folder: 0.0}
            draw_text +: {is_folder: 0.0}
            draw_icon +: {
                color: theme.color_label_inner_inactive
                color_active: theme.color_label_inner_inactive
            }
        }

        folder_node: mod.widgets.FileTreeNode{
            is_folder: true
            draw_bg +: {is_folder: 1.0}
            draw_text +: {is_folder: 1.0}
        }

        filler +: {
            pixel: fn() {
                return mix(
                    mix(
                        theme.color_bg_even
                        theme.color_bg_odd
                        self.is_even
                    )
                    mix(
                        theme.color_outset_inactive
                        theme.color_outset_active
                        self.focussed
                    )
                    self.active
                )
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawBgQuad {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    is_even: f32,
    #[live]
    scale: f32,
    #[live]
    is_folder: f32,
    #[live]
    focussed: f32,
    #[live]
    active: f32,
    #[live]
    hover: f32,
    #[live]
    opened: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawNameText {
    #[deref]
    draw_super: DrawText,
    #[live]
    color_active: Vec4,
    #[live]
    is_even: f32,
    #[live]
    scale: f32,
    #[live]
    is_folder: f32,
    #[live]
    focussed: f32,
    #[live]
    active: f32,
    #[live]
    hover: f32,
    #[live]
    opened: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawIconQuad {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    is_even: f32,
    #[live]
    scale: f32,
    #[live]
    is_folder: f32,
    #[live]
    focussed: f32,
    #[live]
    active: f32,
    #[live]
    hover: f32,
    #[live]
    opened: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawStatusDotQuad {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    status_kind: GitStatusDotKind,
    #[live]
    color_new: Vec4,
    #[live]
    color_modified: Vec4,
    #[live]
    color_deleted: Vec4,
    #[live]
    color_mixed: Vec4,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum GitStatusDotKind {
    #[pick]
    None = 0,
    New = 1,
    Modified = 2,
    Deleted = 3,
    Mixed = 4,
}

/// Everything the tree hands a node right before it draws. `draw_folder` and
/// `draw_file` used to be called directly with these as arguments; the node is
/// a widget now, so the tree parks them here and `draw_walk` consumes them.
pub struct FileTreeNodeDraw {
    pub name: String,
    pub status_kind: GitStatusDotKind,
    pub is_even: f32,
    pub node_height: f64,
    pub depth: usize,
    pub scale: f64,
    pub is_folder: bool,
}

/// A file-tree row is a real widget: it has a uid and sits in the widget tree
/// under its `FileTree`, so the design tweaker can pick it (in 2D and on its
/// own plane in the exploded view) and style its template. The tree still
/// drives it directly — `handle_event_with` for actions, `draw_folder` /
/// `draw_file` for the row — the widget seams only add identity.
#[derive(Script, ScriptHook, Animator, Widget)]
pub struct FileTreeNode {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    /// What the tree parked for the next draw (see `FileTreeNodeDraw`).
    #[rust]
    pub pending_draw: Option<FileTreeNodeDraw>,
    #[redraw]
    #[live]
    draw_bg: DrawBgQuad,
    #[live]
    draw_icon: DrawIconQuad,
    #[live]
    draw_status_dot: DrawStatusDotQuad,
    #[live]
    draw_text: DrawNameText,
    #[layout]
    layout: Layout,

    #[apply_default]
    animator: Animator,

    #[live]
    indent_width: f64,
    #[live]
    indent_shift: f64,

    #[live]
    icon_walk: Walk,
    #[live]
    status_dot_walk: Walk,

    #[live]
    is_folder: bool,
    #[live]
    min_drag_distance: f64,

    #[live]
    opened: f32,
    #[live]
    focussed: f32,
    #[live]
    hover: f32,
    #[live]
    active: f32,
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct FileTree {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[live]
    scroll_bars: ScrollBars,
    #[live]
    file_node: ScriptObjectRef,
    #[live]
    folder_node: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    filler: DrawBgQuad,

    #[live]
    node_height: f64,

    #[live]
    draw_scroll_shadow: DrawScrollShadow,

    #[rust]
    draw_state: DrawStateWrap<()>,

    #[rust]
    dragging_node_id: Option<LiveId>,
    #[rust]
    selected_node_id: Option<LiveId>,
    /// A programmatic selection wants this node scrolled into view on its
    /// next draw (the tweaker's pin → tree sync).
    #[rust]
    scroll_to_pending: Option<LiveId>,
    #[rust]
    reveal_node: Option<LiveId>,
    #[rust]
    reveal_y: Option<f64>,
    #[rust]
    open_nodes: HashSet<LiveId>,

    /// Each row is a widget of its own, registered in the widget tree as this
    /// tree's child under its node id (the design tweaker picks them).
    #[rust]
    tree_nodes: ComponentMap<LiveId, WidgetRef>,

    #[rust]
    count: usize,
    #[rust]
    stack: Vec<f64>,
}

impl ScriptHook for FileTree {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        _value: ScriptValue,
    ) {
        // Apply updates to existing nodes
        if apply.is_reload() {
            for tree_node in self.tree_nodes.values_mut() {
                let Some(mut tree_node) = tree_node.borrow_mut::<FileTreeNode>() else {
                    continue;
                };
                let template = if tree_node.is_folder {
                    self.folder_node.clone()
                } else {
                    self.file_node.clone()
                };
                tree_node.script_apply(vm, apply, scope, template.into());
            }
        }

        vm.with_cx_mut(|cx| {
            self.scroll_bars.redraw(cx);
        });
    }
}

#[derive(Clone, Debug, Default)]
pub enum FileTreeAction {
    #[default]
    None,
    FileClicked(LiveId),
    FolderClicked(LiveId),
    /// A node's row is under the pointer (files and folders alike).
    NodeHovered(LiveId),
    /// The pointer left a node's row.
    NodeHoverEnded(LiveId),
    ShouldFileStartDrag(LiveId),
}

pub enum FileTreeNodeAction {
    WasClicked,
    /// Pointer entered the node's row (inspector-style hover linkage).
    WasHovered,
    /// Pointer left the node's row.
    HoverEnded,
    Opening,
    Closing,
    ShouldStartDrag,
}

impl FileTreeNode {
    pub fn set_draw_state(&mut self, is_even: f32, scale: f64) {
        self.draw_bg.scale = scale as f32;
        self.draw_bg.is_even = is_even;
        self.draw_text.scale = scale as f32;
        self.draw_text.is_even = is_even;
        self.draw_icon.scale = scale as f32;
        self.draw_icon.is_even = is_even;
        self.draw_icon.is_folder = if self.is_folder { 1.0 } else { 0.0 };
        self.draw_text.font_scale = scale as f32;
    }

    pub fn draw_folder(
        &mut self,
        cx: &mut Cx2d,
        name: &str,
        status_kind: GitStatusDotKind,
        is_even: f32,
        node_height: f64,
        depth: usize,
        scale: f64,
    ) {
        self.set_draw_state(is_even, scale);

        self.draw_bg.begin(
            cx,
            Walk::new(Size::fill(), Size::Fixed(scale * node_height)),
            self.layout,
        );

        let show_dot = depth > 0;
        cx.walk_turtle(self.indent_walk(depth, show_dot));

        if show_dot {
            self.draw_status_dot.status_kind = status_kind;
            self.draw_status_dot.draw_walk(cx, self.status_dot_walk);
        }
        self.draw_icon.draw_walk(cx, self.icon_walk);

        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }

    pub fn draw_file(
        &mut self,
        cx: &mut Cx2d,
        name: &str,
        status_kind: GitStatusDotKind,
        is_even: f32,
        node_height: f64,
        depth: usize,
        scale: f64,
    ) {
        self.set_draw_state(is_even, scale);

        self.draw_bg.begin(
            cx,
            Walk::new(Size::fill(), Size::Fixed(scale * node_height)),
            self.layout,
        );

        let show_dot = true;
        cx.walk_turtle(self.indent_walk(depth, depth > 0));
        self.draw_status_dot.status_kind = status_kind;
        if show_dot {
            self.draw_status_dot.draw_walk(cx, self.status_dot_walk);
        }

        self.draw_text
            .draw_walk(cx, Walk::fit(), Align::default(), name);
        self.draw_bg.end(cx);
    }

    fn status_dot_slot_width(&self) -> f64 {
        let width = match self.status_dot_walk.width {
            Size::Fixed(width) => width,
            _ => 0.0,
        };
        width + self.status_dot_walk.margin.left + self.status_dot_walk.margin.right
    }

    fn indent_walk(&self, depth: usize, dot_in_indent: bool) -> Walk {
        let mut width = depth as f64 * self.indent_width + self.indent_shift;
        if dot_in_indent {
            let reclaimed = (self.status_dot_slot_width() - 2.0).max(0.0);
            width = (width - reclaimed).max(0.0);
        }

        Walk {
            abs_pos: None,
            width: Size::Fixed(width),
            height: Size::Fixed(0.0),
            margin: Inset {
                left: depth as f64 * 1.0,
                top: 0.0,
                right: depth as f64 * 4.0,
                bottom: 0.0,
            },
            ..Default::default()
        }
    }

    fn set_is_selected(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, ids!(select.on), ids!(select.off))
    }

    fn set_is_focussed(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, ids!(focus.on), ids!(focus.off))
    }

    pub fn set_folder_is_open(&mut self, cx: &mut Cx, is: bool, animate: Animate) {
        self.animator_toggle(cx, is, animate, ids!(open.on), ids!(open.off));
    }

    pub fn handle_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        node_id: LiveId,
        _scope: &mut Scope,
        actions: &mut Vec<(LiveId, FileTreeNodeAction)>,
    ) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
        match event.hits(cx, self.draw_bg.area()) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
                actions.push((node_id, FileTreeNodeAction::WasHovered));
            }
            Hit::FingerHoverOut(_) => {
                self.animator_play(cx, ids!(hover.off));
                actions.push((node_id, FileTreeNodeAction::HoverEnded));
            }
            Hit::FingerMove(f) => {
                if f.abs.distance(&f.abs_start) >= self.min_drag_distance {
                    actions.push((node_id, FileTreeNodeAction::ShouldStartDrag));
                }
            }
            Hit::FingerDown(_) => {
                self.animator_play(cx, ids!(select.on));
                if self.is_folder {
                    if self.animator_in_state(cx, ids!(open.on)) {
                        self.animator_play(cx, ids!(open.off));
                        actions.push((node_id, FileTreeNodeAction::Closing));
                    } else {
                        self.animator_play(cx, ids!(open.on));
                        actions.push((node_id, FileTreeNodeAction::Opening));
                    }
                }
                actions.push((node_id, FileTreeNodeAction::WasClicked));
            }
            _ => {}
        }
    }
}

impl Widget for FileTreeNode {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
        // Driven by `FileTree::handle_event` through `handle_event_with`, which
        // needs the tree's node id and action sink; nothing to do on the plain
        // seam.
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        if let Some(d) = self.pending_draw.take() {
            if d.is_folder {
                self.draw_folder(
                    cx,
                    &d.name,
                    d.status_kind,
                    d.is_even,
                    d.node_height,
                    d.depth,
                    d.scale,
                );
            } else {
                self.draw_file(
                    cx,
                    &d.name,
                    d.status_kind,
                    d.is_even,
                    d.node_height,
                    d.depth,
                    d.scale,
                );
            }
        }
        DrawStep::done()
    }
}

impl FileTree {
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.scroll_bars.begin(cx, walk, self.layout);
        self.count = 0;
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        // lets fill the space left with blanks
        let height_left = cx.turtle().unused_inner_height();
        let mut walk = 0.0;
        while walk < height_left {
            self.count += 1;
            self.filler.is_even = Self::is_even(self.count);
            let height = self.node_height.min(height_left - walk);
            self.filler
                .draw_walk(cx, Walk::new(Size::fill(), Size::Fixed(height)));
            walk += height.max(1.0);
        }

        self.draw_scroll_shadow.draw(cx, dvec2(0., 0.));
        self.scroll_bars.end(cx);

        let selected_node_id = self.selected_node_id;
        self.tree_nodes
            .retain_visible_and(|node_id, _| Some(*node_id) == selected_node_id);
    }

    pub fn is_even(count: usize) -> f32 {
        if count % 2 == 1 {
            0.0
        } else {
            1.0
        }
    }

    pub fn should_node_draw(&mut self, cx: &mut Cx2d) -> bool {
        let scale = self.stack.last().cloned().unwrap_or(1.0);
        let height = self.node_height * scale;
        let walk = Walk::new(Size::fill(), Size::Fixed(height));
        if scale > 0.01 && cx.walk_turtle_would_be_visible(walk) {
            return true;
        } else {
            cx.walk_turtle(walk);
            return false;
        }
    }

    pub fn begin_folder_with_status(
        &mut self,
        cx: &mut Cx2d,
        node_id: LiveId,
        name: &str,
        status_kind: GitStatusDotKind,
    ) -> Result<(), ()> {
        if self.reveal_node == Some(node_id) {
            self.reveal_node = None;
            self.reveal_y = Some(cx.turtle().pos().y);
        }
        let scale = self.stack.last().cloned().unwrap_or(1.0);

        if scale > 0.2 {
            self.count += 1;
        }

        let is_open = self.open_nodes.contains(&node_id);

        if self.should_node_draw(cx) {
            let tree_node = self.get_or_create_node(cx, node_id, true, is_open);
            if let Some(mut node) = tree_node.borrow_mut::<FileTreeNode>() {
                node.pending_draw = Some(FileTreeNodeDraw {
                    name: name.to_string(),
                    status_kind,
                    is_even: Self::is_even(self.count),
                    node_height: self.node_height,
                    depth: self.stack.len(),
                    scale,
                    is_folder: true,
                });
            }
            // Through the widget seam, so the row counts as its own nesting
            // level and the design tweaker's plane pick lands on it.
            tree_node.draw_all(cx, &mut Scope::empty());
            let opened = tree_node
                .borrow::<FileTreeNode>()
                .map(|node| node.opened)
                .unwrap_or(0.0);
            self.stack.push(opened as f64 * scale);
            if opened <= 0.001 {
                self.end_folder();
                return Err(());
            }
        } else {
            if is_open {
                self.stack.push(scale * 1.0);
            } else {
                return Err(());
            }
        }
        Ok(())
    }

    pub fn begin_folder(&mut self, cx: &mut Cx2d, node_id: LiveId, name: &str) -> Result<(), ()> {
        self.begin_folder_with_status(cx, node_id, name, GitStatusDotKind::None)
    }

    pub fn end_folder(&mut self) {
        self.stack.pop();
    }

    pub fn file_with_status(
        &mut self,
        cx: &mut Cx2d,
        node_id: LiveId,
        name: &str,
        status_kind: GitStatusDotKind,
    ) {
        if self.reveal_node == Some(node_id) {
            self.reveal_node = None;
            self.reveal_y = Some(cx.turtle().pos().y);
        }
        let scale = self.stack.last().cloned().unwrap_or(1.0);

        if scale > 0.2 {
            self.count += 1;
        }
        if self.should_node_draw(cx) {
            let tree_node = self.get_or_create_node(cx, node_id, false, false);
            if let Some(mut node) = tree_node.borrow_mut::<FileTreeNode>() {
                node.pending_draw = Some(FileTreeNodeDraw {
                    name: name.to_string(),
                    status_kind,
                    is_even: Self::is_even(self.count),
                    node_height: self.node_height,
                    depth: self.stack.len(),
                    scale,
                    is_folder: false,
                });
            }
            tree_node.draw_all(cx, &mut Scope::empty());
            if self.scroll_to_pending == Some(node_id) {
                self.scroll_to_pending = None;
                let rect = tree_node.area().rect(cx);
                if rect.size.y > 0.0 {
                    self.scroll_bars.scroll_into_view(cx, rect);
                }
            }
        }
    }

    /// The row widget for `node_id`, created from the folder/file template on
    /// first sight and registered in the widget tree under this tree's uid.
    fn get_or_create_node(
        &mut self,
        cx: &mut Cx2d,
        node_id: LiveId,
        is_folder: bool,
        is_open: bool,
    ) -> WidgetRef {
        let template = if is_folder {
            self.folder_node.clone()
        } else {
            self.file_node.clone()
        };
        let tree_uid = self.uid;
        let selected = self.selected_node_id == Some(node_id);
        self.tree_nodes
            .get_or_insert(cx, node_id, |cx| {
                let tree_node =
                    cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.into()));
                if is_folder && is_open {
                    if let Some(mut node) = tree_node.borrow_mut::<FileTreeNode>() {
                        node.set_folder_is_open(cx, true, Animate::No);
                    }
                }
                if selected {
                    // A programmatic selection can land before the node has
                    // ever drawn: the widget is created selected (and
                    // focussed — the selected bg is gated by the focus mix).
                    if let Some(mut node) = tree_node.borrow_mut::<FileTreeNode>() {
                        node.set_is_selected(cx, true, Animate::No);
                        node.set_is_focussed(cx, true, Animate::No);
                    }
                }
                cx.widget_tree_insert_child(tree_uid, node_id, tree_node.clone());
                tree_node
            })
            .clone()
    }

    /// The current scroll offset (the tweaker's retrying scroll-to needs it).
    pub fn scroll_pos(&self) -> Vec2d {
        self.scroll_bars.get_scroll_pos()
    }

    /// Ask the next draw to report where `node_id`'s row lands on screen
    /// (culled rows report too — their space is walked). The caller reads
    /// `take_reveal_y` after driving the draw and corrects the scroll by
    /// the measured error; a couple of frames converge exactly, whatever
    /// the content height or clamping did.
    pub fn begin_reveal(&mut self, node_id: LiveId) {
        self.reveal_node = Some(node_id);
        self.reveal_y = None;
    }

    pub fn take_reveal_y(&mut self) -> Option<f64> {
        self.reveal_y.take()
    }

    pub fn scroll_by(&mut self, cx: &mut Cx, dy: f64) {
        let now = self.scroll_bars.get_scroll_pos();
        self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(now.x, (now.y + dy).max(0.0)));
    }

    /// Whether a folder node is currently open (the fold state lives here;
    /// the tweaker's scroll-to-selection math needs it).
    pub fn is_folder_open(&self, node_id: LiveId) -> bool {
        self.open_nodes.contains(&node_id)
    }

    /// One row's height in the tree's layout.
    pub fn row_height(&self) -> f64 {
        self.node_height
    }

    /// Scroll the viewport so content at `y` (content coords) is on
    /// screen — the tree virtualises rows, so a draw-driven scroll never
    /// fires for a node that has not drawn.
    pub fn scroll_to_y(&mut self, cx: &mut Cx, y: f64) {
        // no_clip: on the tree's FIRST draw the content height is not yet
        // measured and a clipped set would clamp the target back to zero.
        self.scroll_bars.set_scroll_pos_no_clip(cx, dvec2(0.0, y.max(0.0)));
    }

    /// Select a node programmatically and reveal it (the click path stays
    /// the authority for user selection).
    pub fn select_node(&mut self, cx: &mut Cx, node_id: LiveId) {
        if self.selected_node_id == Some(node_id) {
            return;
        }
        if let Some(last) = self.selected_node_id {
            if let Some(node) = self.tree_nodes.get_mut(&last) {
                if let Some(mut node) = node.borrow_mut::<FileTreeNode>() {
                    node.set_is_selected(cx, false, Animate::No);
                    node.set_is_focussed(cx, false, Animate::No);
                }
            }
        }
        self.selected_node_id = Some(node_id);
        if let Some(node) = self.tree_nodes.get_mut(&node_id) {
            if let Some(mut node) = node.borrow_mut::<FileTreeNode>() {
                node.set_is_selected(cx, true, Animate::No);
                // Present as focussed: the selected bg is gated by the
                // focus mix and reads near-invisible without it.
                node.set_is_focussed(cx, true, Animate::No);
            }
        }
        self.scroll_to_pending = Some(node_id);
        self.scroll_bars.redraw(cx);
    }

    pub fn file(&mut self, cx: &mut Cx2d, node_id: LiveId, name: &str) {
        self.file_with_status(cx, node_id, name, GitStatusDotKind::None);
    }

    pub fn forget(&mut self) {
        self.tree_nodes.clear();
    }

    pub fn forget_node(&mut self, file_node_id: LiveId) {
        self.tree_nodes.remove(&file_node_id);
    }

    pub fn is_folder(&mut self, file_node_id: LiveId) -> bool {
        if let Some(node) = self.tree_nodes.get(&file_node_id) {
            node.borrow::<FileTreeNode>()
                .map(|node| node.is_folder)
                .unwrap_or(false)
        } else {
            false
        }
    }

    pub fn set_folder_is_open(
        &mut self,
        cx: &mut Cx,
        node_id: LiveId,
        is_open: bool,
        animate: Animate,
    ) {
        if is_open {
            self.open_nodes.insert(node_id);
        } else {
            self.open_nodes.remove(&node_id);
        }
        if let Some(tree_node) = self.tree_nodes.get_mut(&node_id) {
            if let Some(mut tree_node) = tree_node.borrow_mut::<FileTreeNode>() {
                tree_node.set_folder_is_open(cx, is_open, animate);
            }
        }
    }

    fn set_selected_is_focussed(&mut self, cx: &mut Cx, is: bool) {
        let Some(node_id) = self.selected_node_id else {
            return;
        };
        let Some(node) = self.tree_nodes.get_mut(&node_id) else {
            return;
        };
        if let Some(mut node) = node.borrow_mut::<FileTreeNode>() {
            node.set_is_focussed(cx, is, Animate::Yes);
        }
    }

    pub fn start_dragging_file_node(&mut self, cx: &mut Cx, node_id: LiveId, items: Vec<DragItem>) {
        self.dragging_node_id = Some(node_id);
        log!("makepad: start_dragging_file_node");
        cx.start_dragging(items);
    }
}

impl WidgetNode for FileTree {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.scroll_bars.area()
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        // The rows are widgets too — surface them so the design tweaker's
        // pick walk reaches them.
        for (node_id, node) in self.tree_nodes.iter() {
            visit(*node_id, node.clone());
        }
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx);
    }
}

impl Widget for FileTree {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();

        self.scroll_bars.handle_event(cx, event, scope);

        match event {
            Event::DragEnd => self.dragging_node_id = None,
            _ => (),
        }

        let mut node_actions = Vec::new();

        for (node_id, node) in self.tree_nodes.iter_mut() {
            let Some(mut node) = node.borrow_mut::<FileTreeNode>() else {
                continue;
            };
            node.handle_event_with(cx, event, *node_id, scope, &mut node_actions);
        }

        for (node_id, node_action) in node_actions {
            match node_action {
                FileTreeNodeAction::Opening => {
                    self.open_nodes.insert(node_id);
                }
                FileTreeNodeAction::Closing => {
                    self.open_nodes.remove(&node_id);
                }
                FileTreeNodeAction::WasHovered => {
                    cx.widget_action(uid, FileTreeAction::NodeHovered(node_id));
                }
                FileTreeNodeAction::HoverEnded => {
                    cx.widget_action(uid, FileTreeAction::NodeHoverEnded(node_id));
                }
                FileTreeNodeAction::WasClicked => {
                    cx.set_key_focus(self.scroll_bars.area());
                    if let Some(last_selected) = self.selected_node_id {
                        if last_selected != node_id {
                            if let Some(node) = self.tree_nodes.get_mut(&last_selected) {
                                if let Some(mut node) = node.borrow_mut::<FileTreeNode>() {
                                    node.set_is_selected(cx, false, Animate::Yes);
                                }
                            }
                        }
                    }
                    self.selected_node_id = Some(node_id);
                    if self.is_folder(node_id) {
                        cx.widget_action(uid, FileTreeAction::FolderClicked(node_id));
                    } else {
                        cx.widget_action(uid, FileTreeAction::FileClicked(node_id));
                    }
                }
                FileTreeNodeAction::ShouldStartDrag => {
                    if self.dragging_node_id.is_none() {
                        cx.widget_action(uid, FileTreeAction::ShouldFileStartDrag(node_id));
                    }
                }
            }
        }

        match event.hits(cx, self.scroll_bars.area()) {
            Hit::KeyFocus(_) => {
                self.set_selected_is_focussed(cx, true);
            }
            Hit::KeyFocusLost(_) => {
                self.set_selected_is_focussed(cx, false);
            }
            _ => (),
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(cx, ()) {
            self.begin(cx, walk);
            return DrawStep::make_step();
        }
        if let Some(()) = self.draw_state.get() {
            self.end(cx);
            self.draw_state.end();
        }
        DrawStep::done()
    }
}

impl FileTreeRef {
    pub fn should_file_start_drag(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileTreeAction::ShouldFileStartDrag(file_id) = item.cast() {
                return Some(file_id);
            }
        }
        None
    }

    pub fn file_clicked(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileTreeAction::FileClicked(file_id) = item.cast() {
                return Some(file_id);
            }
        }
        None
    }

    pub fn folder_clicked(&self, actions: &Actions) -> Option<LiveId> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileTreeAction::FolderClicked(file_id) = item.cast() {
                return Some(file_id);
            }
        }
        None
    }

    pub fn set_folder_is_open(
        &self,
        cx: &mut Cx,
        node_id: LiveId,
        is_open: bool,
        animate: Animate,
    ) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_folder_is_open(cx, node_id, is_open, animate);
        }
    }

    pub fn file_start_drag(&self, cx: &mut Cx, _file_id: LiveId, item: DragItem) {
        cx.start_dragging(vec![item]);
    }
}
