//! SpaceLobbyScreen widget extracted from Robrix for standalone testing.
//!
//! All matrix-sdk dependencies have been removed and replaced with
//! simple standalone types. Fake test data is injected at startup.

use std::collections::{HashMap, HashSet};
use makepad_widgets::*;
use makepad_widgets::animator::Animate;
use crate::avatar::{user_name_first_letter, AvatarWidgetExt, AvatarWidgetRefExt};


script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*


    // A view that draws the hierarchical tree structure lines.
    let DrawTreeLine = set_type_default() do #(DrawTreeLine::script_shader(vm)){
        ..mod.draw.DrawQuad
    }

    mod.widgets.TreeLines = #(TreeLines::register_widget(vm)) {

        width: 0, height: Fill

        draw_bg: DrawTreeLine {
            indent_width: 44.0
            level: 0.0
            is_last: 0.0
            parent_mask: 0.0

            pixel: fn() {
                let pos = self.pos * self.rect_size;
                let indent = self.indent_width;
                let half_indent = indent * 0.6;
                let line_width = 1.0;
                let half_line = 0.5;

                let mut c = vec4(0.0);

                for i in 0..20 {
                    if f32(i) > self.level { break; }

                    if f32(i) < self.level {
                        let mask_bit = modf(floor(self.parent_mask / pow(2.0, f32(i))), 2.0);
                        if mask_bit > 0.5 {
                            if abs(pos.x - (f32(i) * indent + half_indent)) < half_line && pos.y < self.rect_size.y {
                                c = vec4(0.8, 0.8, 0.8, 1.0);
                                break;
                            }
                        }
                    } else {
                        let hy = self.rect_size.y * 0.5;
                        if abs(pos.y - hy) < half_line && pos.x > (f32(i) * indent + half_indent) {
                            c = vec4(0.8, 0.8, 0.8, 1.0);
                            break;
                        }

                        if abs(pos.x - (f32(i) * indent + half_indent)) < half_line && pos.y < (self.rect_size.y * (1.0 - 0.5 * self.is_last)) {
                            c = vec4(0.8, 0.8, 0.8, 1.0);
                            break;
                        }
                    }
                }
                return c;
            }
        }
    }

    // Animated expand/collapse arrow drawn via Sdf2d.
    mod.widgets.ExpandArrowBase = #(ExpandArrow::register_widget(vm))

    mod.widgets.ExpandArrow = set_type_default() do mod.widgets.ExpandArrowBase {
        width: 14, height: 14,

        draw_bg +: {
            opened: instance(0.0)
            color: instance(#888)

            pixel: fn() {
                let sz = 4.0
                let c = vec2(self.rect_size.x * 0.5, self.rect_size.y * 0.5)
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.clear(vec4(0.0))

                // Triangle pointing up; rotation maps opened to:
                //   0.0 -> 90deg (right-pointing, collapsed)
                //   1.0 -> 180deg (down-pointing, expanded)
                sdf.rotate(self.opened * 0.5 * PI + 0.5 * PI, c.x, c.y)
                sdf.move_to(c.x - sz, c.y + sz)
                sdf.line_to(c.x, c.y - sz)
                sdf.line_to(c.x + sz, c.y + sz)
                sdf.close_path()
                sdf.fill(self.color)

                return sdf.result
            }
        }

        animator: Animator{
            expand: {
                default: @collapsed
                collapsed: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    ease: ExpDecay {d1: 0.96, d2: 0.97}
                    redraw: true
                    apply: { draw_bg: {opened: 0.0} }
                }
                expanded: AnimatorState{
                    from: {all: Forward {duration: 0.15}}
                    ease: ExpDecay {d1: 0.98, d2: 0.95}
                    redraw: true
                    apply: { draw_bg: {opened: 1.0} }
                }
            }
        }
    }

    // Entry for a child subspace (can be expanded)
    mod.widgets.SubspaceEntry = set_type_default() do #(SubspaceEntry::register_widget(vm)) {
        ..mod.widgets.SolidView

        width: Fill,
        height: 44,
        flow: Right,
        align: Align{y: 0.5}
        padding: Inset{left: 8, right: 12}
        cursor: MouseCursor.Hand

        show_bg: true
        draw_bg +: {
            hover: instance(0.0)
            color: instance(#fff)
            color_hover: instance(#f5f5f5)
            pixel: fn() {
                return mix(self.color, self.color_hover, self.hover);
            }
        }

        // The connecting hierarchical lines on the left.
        tree_lines := mod.widgets.TreeLines {}

        // Expand/collapse arrow (animated triangle)
        expand_icon := mod.widgets.ExpandArrow {
            width: 14,
            height: 14,
            margin: Inset{ left: -6, right: 4 }
        }

        avatar := Avatar { width: 32, height: 32, margin: Inset{right: 8} }

        content := View {
            width: Fill
            height: Fit
            flow: Down
            align: Align { y: 0.5 }
            spacing: 5,
            name_label := Label {
                width: Fill, height: Fit,
                flow: Flow.Right{wrap: false}
                margin: 0
                padding: 0
                draw_text +: { text_style: REGULAR_TEXT {font_size: 10.5}, color: #1a1a1a }
            }
            info_label := Label {
                width: Fill, height: Fit,
                flow: Flow.Right{wrap: false}
                margin: 0
                padding: 0
                draw_text +: { text_style: REGULAR_TEXT {font_size: 8.5}, color: #737373 }
            }
        }

        buttons_view := View {
            width: Fit,
            height: Fit,
            flow: Right,
            spacing: 8,
            align: Align{x: 1.0, y: 0.5}
            margin: Inset{left: 8}
            visible: false

            join_button := RobrixIconButton {
                width: Fit,
                padding: 8
                spacing: 0
                icon_walk: Walk{width: 0, height: 0}
                draw_bg +: {
                    border_size: 0.75
                    border_color: (COLOR_FG_ACCEPT_GREEN)
                    color: (COLOR_BG_ACCEPT_GREEN)
                }
                draw_text +: {
                    text_style: REGULAR_TEXT {font_size: 9.5},
                    color: (COLOR_FG_ACCEPT_GREEN),
                }
                text: "Join"
            }

            view_button := RobrixIconButton {
                width: Fit,
                padding: 8
                spacing: 0
                icon_walk: Walk{width: 0, height: 0}
                draw_bg +: {
                    border_size: 0.0
                    color: (COLOR_ACTIVE_PRIMARY)
                }
                draw_text +: {
                    text_style: REGULAR_TEXT {font_size: 9.5},
                    color: (COLOR_TEXT),
                }
                text: "View"
            }

            leave_button := RobrixIconButton {
                width: Fit,
                padding: 8
                spacing: 0
                icon_walk: Walk{width: 0, height: 0}
                draw_bg +: {
                    border_size: 0.75
                    border_color: (COLOR_FG_DANGER_RED)
                    color: (COLOR_BG_DANGER_RED)
                }
                draw_text +: {
                    text_style: REGULAR_TEXT {font_size: 9.5},
                    color: (COLOR_FG_DANGER_RED),
                }
                text: "Leave"
            }
        }


        animator: Animator{
            hover: {
                default: @off
                off: AnimatorState{ from: {all: Forward {duration: 0.1}}, apply: { draw_bg: {hover: 0.0} } }
                on: AnimatorState{ from: {all: Snap}, apply: { draw_bg: {hover: 1.0} } }
            }
        }
    }

    // Entry for a child room within a space, which cannot be expanded.
    mod.widgets.RoomEntry = mod.widgets.SubspaceEntry {
        cursor: MouseCursor.Default

        expand_icon := View {
            width: 10
            height: 16
        }
    }

    mod.widgets.SpaceLobbyStatusLabel = View {
        width: Fill, height: Fit,
        flow: Right,
        align: Align{ x: 0.5, y: 0.5 }
        padding: 20.0,

        loading_spinner := LoadingSpinner {
            width: 18,
            height: 18,
            draw_bg +: {
                color: (COLOR_ACTIVE_PRIMARY)
                border_size: 2.5
            }
        }

        label := Label {
            padding: Inset{left: 10}
            width: Fit,
            flow: Flow.Right{wrap: true},
            align: Align{ x: 0.5, y: 0.5 }
            draw_text +: {
                flow: Flow.Right{wrap: true},
                color: #737373,
                text_style: REGULAR_TEXT {font_size: 10}
            }
            text: "Loading rooms and spaces..."
        }
    }

    // Small loading indicator shown inline when loading subspace children
    mod.widgets.SubspaceLoadingEntry = View {
        width: Fill, height: 36,
        flow: Right,
        align: Align{ y: 0.5 }
        padding: Inset{left: 8, right: 12}

        tree_lines := mod.widgets.TreeLines {}

        loading_spinner := LoadingSpinner {
            width: 14,
            height: 14,
            margin: Inset{left: 8, right: 10}
            draw_bg +: {
                color: (COLOR_ACTIVE_PRIMARY)
                border_size: 2.0
            }
        }

        label := Label {
            width: Fit,
            height: Fit,
            draw_text +: {
                text_style: REGULAR_TEXT {font_size: 9},
                color: #888,
            }
            text: "Loading..."
        }
    }

    // The main view that shows the lobby (homepage) for a space.
    mod.widgets.SpaceLobbyScreen = set_type_default() do #(SpaceLobbyScreen::register_widget(vm)) {
        ..mod.widgets.SolidView

        width: Fill, height: Fill,
        flow: Down,

        show_bg: true
        draw_bg.color: #fff

        // Header with parent space info
        header := SolidView {
            width: Fill,
            height: Fit,
            flow: Down,
            padding: Inset{left: 16, right: 16, top: 16, bottom: 8}

            show_bg: true,
            draw_bg.color: (COLOR_BG_PREVIEW)

            space_info_label := Label {
                width: Fill,
                height: Fit,
                margin: Inset{left: 2}
                draw_text +: {
                    text_style: REGULAR_TEXT {font_size: 10},
                    color: #737373,
                    flow: Flow.Right{wrap: true},
                }
                text: "Welcome to the space:"
            }

            parent_space_row := View {
                width: Fill,
                height: Fit,
                flow: Right,
                align: Align{ y: 0.5 }
                padding: Inset{ top: 8 }

                parent_avatar := Avatar {
                    width: 36,
                    height: 36,
                    margin: Inset{ right: 12 }
                }

                parent_name := Label {
                    width: Fill,
                    height: Fit,
                    margin: Inset{top: 4}
                    draw_text +: {
                        text_style: TITLE_TEXT {font_size: 14},
                        color: #1a1a1a,
                        flow: Flow.Right{wrap: true},
                    }
                    text: ""
                }

                invite_button := RobrixIconButton {
                    width: Fit
                    align: Align{x: 0.5, y: 0.5}
                    margin: Inset{left: 6}
                    padding: 12,
                    draw_icon +: {
                        svg: (mod.widgets.ICON_ADD_USER)
                        color: (COLOR_FG_ACCEPT_GREEN),
                    }
                    icon_walk: Walk{width: 16, height: 16, margin: Inset{left: -2, right: -1} }

                    draw_bg +: {
                        border_size: 0.75
                        border_color: (COLOR_FG_ACCEPT_GREEN)
                        color: (COLOR_BG_ACCEPT_GREEN)
                    }
                    text: "Invite"
                    draw_text +: {
                        color: (COLOR_FG_ACCEPT_GREEN),
                    }
                }
            }
        }

        // The hierarchical tree list
        tree_list := PortalList {
            keep_invisible: false,
            max_pull_down: 0.0,
            auto_tail: false,
            width: Fill, height: Fill
            flow: Down,
            spacing: 0.0

            subspace_entry := mod.widgets.SubspaceEntry {}
            room_entry := mod.widgets.RoomEntry {}
            subspace_loading := mod.widgets.SubspaceLoadingEntry {}
            status_label := mod.widgets.SpaceLobbyStatusLabel {}
            bottom_filler := View {
                width: Fill,
                height: 80.0,
            }
        }
    }
}


// ============================================================================
// Standalone types replacing matrix-sdk dependencies
// ============================================================================

/// Simplified room state (replaces matrix_sdk::RoomState).
#[derive(Clone, Debug, PartialEq)]
pub enum RoomState {
    Joined,
    Left,
    Invited,
}

/// Info about a room/space in the tree (replaces SpaceRoomInfo from Robrix).
#[derive(Clone, Debug)]
pub struct SpaceRoomInfo {
    pub id: String,
    pub name: String,
    pub topic: Option<String>,
    pub num_joined_members: u64,
    pub state: Option<RoomState>,
    /// If `Some`, this is a space. If `None`, it's a room.
    pub children_count: Option<u64>,
}
impl SpaceRoomInfo {
    fn is_space(&self) -> bool {
        self.children_count.is_some()
    }
}

/// An entry in the tree to be displayed.
enum TreeEntry {
    Item {
        info: SpaceRoomInfo,
        level: usize,
        is_last: bool,
        parent_mask: u32,
    },
    Loading {
        level: usize,
        parent_mask: u32,
    },
}


// ============================================================================
// Widget definitions
// ============================================================================

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTreeLine {
    #[deref] draw_super: DrawQuad,
    #[live] indent_width: f32,
    #[live] level: f32,
    #[live] is_last: f32,
    #[live] parent_mask: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TreeLines {
    #[redraw] #[live] draw_bg: DrawTreeLine,
    #[walk] walk: Walk,
}

impl Widget for TreeLines {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) { }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let indent_pixel = (self.draw_bg.level + 1.0) * self.draw_bg.indent_width;
        let mut walk = walk;
        walk.width = Size::Fixed(indent_pixel as f64);
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}


/// Animated expand/collapse triangle arrow.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct ExpandArrow {
    #[source] source: ScriptObjectRef,
    #[apply_default] animator: Animator,
    #[redraw] #[live] draw_bg: DrawQuad,
    #[walk] walk: Walk,
}

impl ExpandArrow {
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.animator_toggle(cx, is_open, animate, ids!(expand.expanded), ids!(expand.collapsed))
    }
}

impl Widget for ExpandArrow {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.draw_bg.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}


/// A clickable entry for a child subspace or room.
#[derive(Script, ScriptHook, Widget, Animator)]
pub struct SubspaceEntry {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,
    #[apply_default] animator: Animator,
    #[rust] room_id: Option<String>,
    #[rust] is_space: bool,
    #[rust] show_buttons_view: bool,
    #[rust] is_expanded: bool,
}

/// Actions emitted when a `SubspaceEntry` or its buttons are clicked.
#[derive(Clone, Debug, Default)]
pub enum SubspaceEntryAction {
    SpaceClicked { space_id: String },
    RoomClicked  { room_id: String },
    JoinClicked  { room_id: String, is_space: bool },
    LeaveClicked { room_id: String, is_space: bool },
    ViewClicked  { room_id: String },
    #[default]
    None,
}

impl ActionDefaultRef for SubspaceEntryAction {
    fn default_ref() -> &'static Self {
        static DEFAULT: SubspaceEntryAction = SubspaceEntryAction::None;
        &DEFAULT
    }
}

impl Widget for SubspaceEntry {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.animator_handle_event(cx, event).must_redraw() {
            self.redraw(cx);
        }

        // NOTE: Use child_by_path instead of widget tree-based lookups
        //       (e.g., self.view.view(), self.view.button()) because these
        //       fail for portal list items.
        let buttons_view_ref = self.view.child_by_path(&[live_id!(buttons_view)]);
        let buttons_view_rect = buttons_view_ref.area().rect(cx);
        let are_buttons_visible = self.show_buttons_view;
        match event.hits_with_test(cx, self.view.area(), |abs, rect, _| {
            rect.contains(abs) && !(are_buttons_visible && buttons_view_rect.contains(abs))
        }) {
            Hit::FingerHoverIn(_) => {
                self.animator_play(cx, ids!(hover.on));
                if !self.show_buttons_view {
                    self.show_buttons_view = true;
                    self.view.child_by_path(&[live_id!(buttons_view)]).set_visible(cx, true);
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOver(_) if !self.show_buttons_view => {
                self.animator_play(cx, ids!(hover.on));
                self.show_buttons_view = true;
                self.view.child_by_path(&[live_id!(buttons_view)]).set_visible(cx, true);
                self.redraw(cx);
            }
            Hit::FingerHoverOut(fe) => {
                let entry_rect = self.view.area().rect(cx);
                let is_over_buttons_view = self.show_buttons_view && buttons_view_rect.contains(fe.abs);
                if !entry_rect.contains(fe.abs) && !is_over_buttons_view {
                    self.animator_play(cx, ids!(hover.off));
                    self.show_buttons_view = false;
                    self.view.child_by_path(&[live_id!(buttons_view)]).set_visible(cx, false);
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(_) => {
                cx.set_key_focus(self.view.area());
            }
            Hit::FingerUp(fe) if fe.is_over && fe.is_primary_hit() && fe.was_tap() => {
                let is_within_buttons_view = self.show_buttons_view
                    && self.view.child_by_path(&[live_id!(buttons_view)]).area().rect(cx).contains(fe.abs);
                if !is_within_buttons_view {
                    if let Some(room_id) = self.room_id.as_ref() {
                        if self.is_space {
                            // Toggle expansion and animate the arrow
                            self.is_expanded = !self.is_expanded;
                            if let Some(mut arrow) = self.view.child_by_path(&[live_id!(expand_icon)]).borrow_mut::<ExpandArrow>() {
                                arrow.set_is_open(cx, self.is_expanded, Animate::Yes);
                            }
                            cx.widget_action(
                                self.widget_uid(),
                                SubspaceEntryAction::SpaceClicked { space_id: room_id.clone() },
                            );
                        } else {
                            cx.widget_action(
                                self.widget_uid(),
                                SubspaceEntryAction::RoomClicked { room_id: room_id.clone() },
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            let join_button = self.view.child_by_path(&[live_id!(buttons_view), live_id!(join_button)]).as_button();
            let leave_button = self.view.child_by_path(&[live_id!(buttons_view), live_id!(leave_button)]).as_button();
            let view_button = self.view.child_by_path(&[live_id!(buttons_view), live_id!(view_button)]).as_button();

            if join_button.clicked(actions) {
                if let Some(room_id) = self.room_id.clone() {
                    log!("Join button clicked for entry: {room_id} (is_space: {})", self.is_space);
                    join_button.reset_hover(cx);
                    cx.widget_action(
                        self.widget_uid(),
                        SubspaceEntryAction::JoinClicked { room_id, is_space: self.is_space },
                    );
                }
            }
            if leave_button.clicked(actions) {
                if let Some(room_id) = self.room_id.clone() {
                    log!("Leave button clicked for entry: {room_id} (is_space: {})", self.is_space);
                    leave_button.reset_hover(cx);
                    cx.widget_action(
                        self.widget_uid(),
                        SubspaceEntryAction::LeaveClicked { room_id, is_space: self.is_space },
                    );
                }
            }
            if view_button.clicked(actions) {
                if let Some(room_id) = self.room_id.clone() {
                    log!("View button clicked for entry: {room_id}");
                    view_button.reset_hover(cx);
                    cx.widget_action(
                        self.widget_uid(),
                        SubspaceEntryAction::ViewClicked { room_id },
                    );
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}


/// The view showing the lobby/homepage for a given space.
#[derive(Script, ScriptHook, Widget)]
pub struct SpaceLobbyScreen {
    #[source] source: ScriptObjectRef,
    #[deref] view: View,

    /// The name of the space currently being displayed.
    #[rust] space_name: Option<String>,

    /// Cache of children for each space we've loaded.
    /// Key is the space_id, value is the list of its direct children.
    #[rust] children_cache: HashMap<String, Vec<SpaceRoomInfo>>,

    /// The set of space IDs that are currently expanded.
    #[rust] expanded_spaces: HashSet<String>,

    /// The ordered list of entries to display in the tree.
    #[rust] tree_entries: Vec<TreeEntry>,

    /// The set of space IDs that are currently "loading" their children.
    #[rust] loading_subspaces: HashSet<String>,

    /// Whether we are currently loading the initial data.
    #[rust] is_loading: bool,
}

impl Widget for SpaceLobbyScreen {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Event::Actions(actions) = event {
            for action in actions {
                // Handle SubspaceEntry clicks
                match action.as_widget_action().cast_ref() {
                    SubspaceEntryAction::SpaceClicked { space_id } => {
                        self.toggle_space_expansion(cx, space_id);
                    }
                    SubspaceEntryAction::RoomClicked { room_id } => {
                        log!("Room clicked: {room_id}");
                    }
                    SubspaceEntryAction::JoinClicked { room_id, is_space } => {
                        log!("Join clicked: {room_id} (is_space: {is_space})");
                    }
                    SubspaceEntryAction::LeaveClicked { room_id, is_space } => {
                        log!("Leave clicked: {room_id} (is_space: {is_space})");
                    }
                    SubspaceEntryAction::ViewClicked { room_id } => {
                        log!("View clicked: {room_id}");
                    }
                    SubspaceEntryAction::None => { }
                }
            }

            // Handle the invite button
            if self.view.button(cx, ids!(header.parent_space_row.invite_button)).clicked(actions) {
                log!("Invite button clicked for space: {:?}", self.space_name);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Draw parent avatar as text initials
        let parent_avatar_ref = self.view.avatar(cx, ids!(parent_avatar));
        let first_char = self.space_name.as_deref()
            .and_then(|name| user_name_first_letter(name));
        parent_avatar_ref.show_text(cx, None, first_char.unwrap_or("S"));

        while let Some(widget_to_draw) = self.view.draw_walk(cx, scope, walk).step() {
            let portal_list_ref = widget_to_draw.as_portal_list();
            let Some(mut list) = portal_list_ref.borrow_mut() else { continue };

            let entry_count = self.tree_entries.len();
            let total_count = if self.is_loading || entry_count == 0 {
                2 // status label + filler
            } else {
                entry_count + 1 // entries + filler
            };

            list.set_item_range(cx, 0, total_count);

            while let Some(item_id) = list.next_visible_item(cx) {
                // Draw loading indicator
                let item = if self.is_loading && item_id == 0 {
                    let item = list.item(cx, item_id, id!(status_label));
                    item.child_by_path(&[live_id!(label)]).as_label().set_text(cx, "Loading rooms and spaces...");
                    item
                }
                // No entries found
                else if entry_count == 0 && item_id == 0 {
                    let item = list.item(cx, item_id, id!(status_label));
                    item.child_by_path(&[live_id!(label)]).as_label().set_text(cx, "No rooms or spaces found.");
                    item.child_by_path(&[live_id!(loading_spinner)]).set_visible(cx, false);
                    item
                }
                // Draw a regular entry
                else if let Some(entry) = self.tree_entries.get(item_id) {
                    match entry {
                        TreeEntry::Item { info, level, is_last, parent_mask } => {
                            let show_join_button = !matches!(info.state, Some(RoomState::Joined));
                            let show_leave_button = !show_join_button;
                            let show_view_button = show_leave_button && !info.is_space();
                            let item = if info.is_space() {
                                let item = list.item(cx, item_id, id!(subspace_entry));
                                let is_expanded = self.expanded_spaces.contains(&info.id);
                                let mut show_buttons_view = false;
                                let mut need_snap = false;
                                if let Some(mut inner) = item.borrow_mut::<SubspaceEntry>() {
                                    let id_changed = inner.room_id.as_ref() != Some(&info.id);
                                    need_snap = id_changed || inner.is_expanded != is_expanded;
                                    inner.room_id = Some(info.id.clone());
                                    inner.is_space = true;
                                    inner.is_expanded = is_expanded;
                                    if id_changed {
                                        inner.show_buttons_view = false;
                                    }
                                    show_buttons_view = inner.show_buttons_view;
                                }
                                item.child_by_path(&[live_id!(buttons_view)]).set_visible(cx, show_buttons_view);
                                // Snap expand arrow to correct state without animation
                                // when item is reused or state changed externally
                                if need_snap {
                                    if let Some(mut arrow) = item.child_by_path(&[live_id!(expand_icon)]).borrow_mut::<ExpandArrow>() {
                                        arrow.set_is_open(cx, is_expanded, Animate::No);
                                    }
                                }
                                item
                            } else {
                                let item = list.item(cx, item_id, id!(room_entry));
                                let mut show_buttons_view = false;
                                if let Some(mut inner) = item.borrow_mut::<SubspaceEntry>() {
                                    let id_changed = inner.room_id.as_ref() != Some(&info.id);
                                    inner.room_id = Some(info.id.clone());
                                    inner.is_space = false;
                                    if id_changed {
                                        inner.show_buttons_view = false;
                                    }
                                    show_buttons_view = inner.show_buttons_view;
                                }
                                item.child_by_path(&[live_id!(buttons_view)]).set_visible(cx, show_buttons_view);
                                item
                            };

                            item.child_by_path(&[live_id!(buttons_view), live_id!(join_button)]).set_visible(cx, show_join_button);
                            item.child_by_path(&[live_id!(buttons_view), live_id!(leave_button)]).set_visible(cx, show_leave_button);
                            item.child_by_path(&[live_id!(buttons_view), live_id!(view_button)]).set_visible(cx, show_view_button);

                            // @rik: here, if you use `item.label()`, it doesn't work.
                            // item.label(cx, ids!(content.name_label)).set_text(cx, &info.name);
                            item.child_by_path(ids!(content.name_label)).as_label().set_text(cx, &info.name);

                            // Show text initials avatar
                            let avatar_ref = item.child_by_path(&[live_id!(avatar)]).as_avatar();
                            let first_char = user_name_first_letter(&info.name);
                            avatar_ref.show_text(cx, None, first_char.unwrap_or("#"));

                            if let Some(mut lines) = item.child_by_path(&[live_id!(tree_lines)]).borrow_mut::<TreeLines>() {
                                lines.draw_bg.level = *level as f32;
                                lines.draw_bg.is_last = if *is_last { 1.0 } else { 0.0 };
                                lines.draw_bg.parent_mask = *parent_mask as f32;
                                lines.draw_bg.indent_width = 44.0;
                            }

                            // Build the info label
                            let info_label = item.child_by_path(&[live_id!(content), live_id!(info_label)]).as_label();
                            let mut info_parts = Vec::new();

                            if let Some(state) = &info.state {
                                match state {
                                    RoomState::Joined => info_parts.push("✅ Joined".to_string()),
                                    RoomState::Left => info_parts.push("Left".to_string()),
                                    RoomState::Invited => info_parts.push("Invited".to_string()),
                                }
                            }

                            info_parts.push(format!(
                                "{} {}",
                                info.num_joined_members,
                                if info.num_joined_members == 1 { "member" } else { "members" }
                            ));

                            if let Some(c) = info.children_count {
                                if c > 0 {
                                    info_parts.push(format!(
                                        "~{} {}",
                                        c,
                                        if c == 1 { "room" } else { "rooms" }
                                    ));
                                }
                            }

                            if let Some(topic) = &info.topic {
                                info_parts.push(topic.to_string());
                            }

                            info_label.set_text(cx, &info_parts.join("  |  "));

                            item
                        }
                        TreeEntry::Loading { level, parent_mask } => {
                            let item = list.item(cx, item_id, id!(subspace_loading));
                            if let Some(mut lines) = item.child_by_path(&[live_id!(tree_lines)]).borrow_mut::<TreeLines>() {
                                lines.draw_bg.level = *level as f32;
                                lines.draw_bg.is_last = 1.0;
                                lines.draw_bg.parent_mask = *parent_mask as f32;
                                lines.draw_bg.indent_width = 44.0;
                            }
                            item
                        }
                    }
                } else {
                    list.item(cx, item_id, id!(bottom_filler))
                };
                item.draw_all(cx, scope);
            }
        }

        DrawStep::done()
    }
}

impl SpaceLobbyScreen {
    /// Toggle the expansion state of a space.
    fn toggle_space_expansion(&mut self, cx: &mut Cx, space_id: &str) {
        if self.expanded_spaces.contains(space_id) {
            self.expanded_spaces.remove(space_id);
            self.loading_subspaces.remove(space_id);
        } else {
            self.expanded_spaces.insert(space_id.to_string());
            // In the real app, this would request children from the server.
            // Here we just check if we already have them cached.
        }

        self.rebuild_tree_entries();
        self.redraw(cx);
    }

    /// Rebuild the flattened tree entries based on the current expansion state.
    fn rebuild_tree_entries(&mut self) {
        let Some(root_space_id) = self.space_name.as_ref().map(|_| "root".to_string()) else { return };
        let mut new_tree_entries = Vec::new();
        Self::build_tree_for_space(
            &self.children_cache,
            &self.expanded_spaces,
            &self.loading_subspaces,
            &mut new_tree_entries,
            &root_space_id,
            0,
            0,
        );
        self.tree_entries = new_tree_entries;
    }

    /// Recursively build the tree.
    fn build_tree_for_space(
        children_cache: &HashMap<String, Vec<SpaceRoomInfo>>,
        expanded_spaces: &HashSet<String>,
        loading_subspaces: &HashSet<String>,
        tree_entries: &mut Vec<TreeEntry>,
        space_id: &str,
        level: usize,
        parent_mask: u32,
    ) {
        let Some(children) = children_cache.get(space_id) else { return };

        // Sort: spaces first, then rooms, both alphabetically
        let mut sorted_children: Vec<_> = children.iter().collect();
        sorted_children.sort_by(|a, b| {
            match (a.is_space(), b.is_space()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        let count = sorted_children.len();
        for (i, child) in sorted_children.into_iter().enumerate() {
            let is_last = i == count - 1;

            tree_entries.push(TreeEntry::Item {
                info: child.clone(),
                level,
                is_last,
                parent_mask,
            });

            // If this is an expanded space, recursively add its children
            if child.is_space() && expanded_spaces.contains(&child.id) {
                let child_mask = if is_last {
                    parent_mask
                } else {
                    parent_mask | (1 << level)
                };

                if children_cache.contains_key(&child.id) {
                    Self::build_tree_for_space(
                        children_cache,
                        expanded_spaces,
                        loading_subspaces,
                        tree_entries,
                        &child.id,
                        level + 1,
                        child_mask,
                    );
                } else if loading_subspaces.contains(&child.id) {
                    tree_entries.push(TreeEntry::Loading {
                        level: level + 1,
                        parent_mask: child_mask,
                    });
                }
            }
        }
    }

    /// Set up the space with the given name and pre-populate with fake data.
    pub fn set_displayed_space(&mut self, cx: &mut Cx, space_name: &str, children_cache: HashMap<String, Vec<SpaceRoomInfo>>) {
        self.space_name = Some(space_name.to_string());
        self.children_cache = children_cache;
        self.is_loading = false;
        self.expanded_spaces.clear();

        // Auto-expand root
        self.expanded_spaces.insert("root".to_string());

        self.view.label(cx, ids!(header.parent_space_row.parent_name)).set_text(cx, space_name);
        self.view.label(cx, ids!(header.space_info_label)).set_text(cx,
            &format!("🌐  Public space  ·  1234 members"),
        );

        self.rebuild_tree_entries();
        self.redraw(cx);
    }
}

impl SpaceLobbyScreenRef {
    pub fn set_displayed_space(&self, cx: &mut Cx, space_name: &str, children_cache: HashMap<String, Vec<SpaceRoomInfo>>) {
        let Some(mut inner) = self.borrow_mut() else { return };
        inner.set_displayed_space(cx, space_name, children_cache);
    }
}
