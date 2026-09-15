//! One state-owning `RemindersView` for wide and compact layouts.

use crate::engine::{
    apply, blank_draft, compact_nav_step, completed_tile_height, create_defaults, decide_layout,
    flatten_rows, home_create_defaults, personal_row_height, project, row_height,
    smart_tile_height, Command, CommandError, CompactNavOp, LayoutMode, ListRow, NavState,
    Projection, MINUTE_TICK_SECS,
};
use crate::model::*;
use crate::seed::seed;
use crate::storage::{
    encode, LoadOutcome, StorageAction, StorageEvent, StorageMachine, StoragePhase,
};
use makepad_ai_services::wire::{ServiceCall, ToolResult};
use makepad_widgets::makepad_platform::script::timer::script_local_utc_offset_secs;
use makepad_widgets::makepad_platform::storage::{
    StorageHandle, StorageOp, StorageRequestId, StorageResponse, StorageResult,
};
use makepad_widgets::makepad_platform::trap::NoTrap;
use makepad_widgets::*;

// Apply partial runtime values in the root's VM. The animation apply mode
// preserves widget/shader sources and updates existing shader values directly.
macro_rules! apply_owned {
    ($cx:ident, $source:expr, $target:expr, $body:tt) => {
        if let Some(vm_id) = $cx.script_ref_vm_id(&$source) {
            $cx.with_script_vm_id(vm_id, |vm| {
                let mut script = script! {
                    use mod.prelude.widgets.*
                    $body
                };
                script.line = line!() as usize;
                script.column = column!() as usize;
                let value = vm.eval(script);
                $target.script_apply(vm, &Apply::Animate, &mut Scope::empty(), value);
            });
        }
    };
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.reminders = {}
    mod.reminders.bg = theme.color_bg_app
    mod.reminders.sidebar = theme.color_bg_container
    mod.reminders.card = theme.color_fg_app
    mod.reminders.ink = theme.color_text
    mod.reminders.ink_secondary = theme.color_text_meta
    mod.reminders.selected = theme.color_bg_highlight
    mod.reminders.today = theme.color_focus
    mod.reminders.scheduled = theme.color_error
    mod.reminders.flagged = theme.color_warning
    mod.reminders.groceries = theme.color_success
    mod.reminders.work = theme.color_focus
    mod.reminders.home = theme.color_warning
    mod.reminders.travel = mix(theme.color_focus, theme.color_error, 0.45)
    mod.reminders.all = mix(theme.color_bg_container, theme.color_text, 0.22)
    mod.reminders.completed = mix(theme.color_bg_container, theme.color_text, 0.38)
    mod.reminders.on_accent = theme.color_text_on_accent

    let Hit = ButtonFlat{
        width: Fill height: Fill text: "" margin: 0 padding: 0
        draw_bg +: {color: #0000 color_hover: #0000 color_down: #0000 color_focus: #0000 border_size: 0.0}
        draw_text +: {color: #0000}
    }

    let SmartTile = RoundedView{
        width: Fill height: 76
        flow: Overlay
        padding: 0
        draw_bg +: {border_radius: 12 border_size: 0.0 color: mod.reminders.today}
        hit := Hit{}
        body := View{
            width: Fill height: Fill flow: Down padding: 10 spacing: 0
            top := View{
                width: Fill height: 36 flow: Right align: Align{y: 0.5}
                symbol := Icon{width: 22 height: 22 icon_walk: Walk{width: 22 height: 22} draw_icon +: {color: mod.reminders.on_accent}}
                View{width: Fill}
                count := Label{padding: 0
                    draw_text +: {color: mod.reminders.on_accent text_style: theme.font_bold{font_size: 26}}
                }
            }
            name := Label{padding: 0
                width: Fill height: Fit
                draw_text +: {color: mod.reminders.on_accent text_style: theme.font_regular{font_size: 13}}
            }
        }
    }

    let ListRowBtn = View{
        width: Fill height: 44 flow: Overlay
        show_bg: true
        draw_bg +: {color: #0000}
        hit := Hit{}
        row := View{
            width: Fill height: Fill flow: Right padding: Inset{left: 8 right: 12} align: Align{y: 0.5} spacing: 10
            icon := Icon{width: 28 height: 28 icon_walk: Walk{width: 22 height: 22} draw_icon +: {color: mod.reminders.groceries}}
            name := Label{padding: 0 width: Fill draw_text +: {color: mod.reminders.ink text_style: theme.font_regular{font_size: 14}}}
            count := Label{padding: 0 draw_text +: {color: mod.reminders.ink_secondary text_style: theme.font_regular{font_size: 14}}}
        }
    }

    let ReminderRow = View{
        width: Fill height: Fit
        flow: Right
        complete := View{
            width: 44 height: 44 flow: Overlay
            align: Align{x: 0.5 y: 0.5}
            hit := Hit{}
            empty := View{
                width: 20 height: 20
                Icon{width: 20 height: 20 icon_walk: Walk{width: 20 height: 20} draw_icon +: {svg: crate_resource("self:resources/icons/circle.svg") color: mod.reminders.ink_secondary}}
            }
            checked := View{
                visible: false width: 20 height: 20
                Icon{width: 20 height: 20 icon_walk: Walk{width: 20 height: 20} draw_icon +: {svg: crate_resource("self:resources/icons/check.svg") color: theme.color_focus}}
            }
        }
        text := View{
            width: Fill height: Fit flow: Down padding: Inset{top: 8 bottom: 8 right: 8}
            title := Label{width: Fill padding: 0 max_lines: 2 text_overflow: Ellipsis draw_text +: {color: mod.reminders.ink wrap: Words text_style: theme.font_regular{font_size: 15}}}
            notes := Label{width: Fill padding: 0 height: Fit max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.reminders.ink_secondary wrap: Words text_style: theme.font_regular{font_size: 12}}}
            metadata := Label{width: Fill padding: 0 height: Fit max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.reminders.ink_secondary wrap: Words text_style: theme.font_regular{font_size: 12}}}
        }
        details := ButtonFlat{
            width: 44 height: 44 margin: 0 padding: 0 text: ""
            icon_walk: Walk{width: 18 height: 18}
            draw_icon +: {svg: crate_resource("self:resources/icons/info.svg") color: mod.reminders.ink_secondary}
            draw_bg +: {color: #0000 color_hover: #0000 color_down: #0000 border_size: 0.0}
        }
    }

    let Composer = View{
        width: Fill height: 44 flow: Right spacing: 8
        plus := Icon{width: 22 height: 22 icon_walk: Walk{width: 18 height: 18} draw_icon +: {svg: crate_resource("self:resources/icons/plus.svg") color: mod.reminders.ink_secondary}}
        title := TextInputFlat{width: Fill height: 44 empty_text: "New Reminder" margin: 0}
        add := ButtonFlat{width: 60 height: 44 margin: 0 text: "Add"}
    }

    let HomeContents = ScrollYView{
        width: Fill height: Fill flow: Down padding: Inset{left: 16 right: 16 top: 12 bottom: 88}
        heading := Label{
            width: Fill height: 44 padding: 0
            text: "Reminders"
            draw_text +: {color: mod.reminders.ink text_style: theme.font_bold{font_size: 32}}
        }
        first := View{
            flow: Right height: Fit spacing: 12 margin: Inset{top: 16}
            today := SmartTile{body.top.symbol.draw_icon.svg: crate_resource("self:resources/icons/calendar_day.svg") body.name.text: "Today"}
            scheduled := SmartTile{
                draw_bg.color: mod.reminders.scheduled
                body.top.symbol.draw_icon.svg: crate_resource("self:resources/icons/calendar.svg")
                body.name.text: "Scheduled"
            }
        }
        second := View{
            flow: Right height: Fit spacing: 12 margin: Inset{top: 12}
            all := SmartTile{
                draw_bg.color: mod.reminders.all
                body.top.symbol.draw_icon.svg: crate_resource("self:resources/icons/tray.svg")
                body.name.text: "All"
            }
            flagged := SmartTile{
                draw_bg.color: mod.reminders.flagged
                body.top.symbol.draw_icon.svg: crate_resource("self:resources/icons/flag.svg")
                body.name.text: "Flagged"
            }
        }
        completed := RoundedView{
            width: Fill height: 52 margin: Inset{top: 10} flow: Overlay
            draw_bg +: {color: mod.reminders.completed border_radius: 12}
            hit := Hit{}
            body := View{
                width: Fill height: Fill padding: Inset{left: 12 right: 12}
                top := View{
                    width: Fill height: Fill flow: Right align: Align{y: 0.5} spacing: 10
                    symbol := Icon{width: 22 height: 22 icon_walk: Walk{width: 22 height: 22} draw_icon +: {svg: crate_resource("self:resources/icons/completed.svg") color: mod.reminders.on_accent}}
                    name := Label{padding: 0 width: Fill text: "Completed" draw_text +: {color: mod.reminders.on_accent text_style: theme.font_regular{font_size: 13}}}
                    count := Label{padding: 0 draw_text +: {color: mod.reminders.on_accent text_style: theme.font_bold{font_size: 26}}}
                }
            }
        }
        my_lists := Label{
            width: Fill height: 28 margin: Inset{top: 20}
            text: "My Lists"
            draw_text +: {color: mod.reminders.ink_secondary text_style: theme.font_regular{font_size: 13}}
        }
        lists := RoundedView{
            width: Fill height: Fit flow: Down
            draw_bg +: {color: mod.reminders.card border_radius: 16}
            groceries := ListRowBtn{row.icon.draw_icon.svg: crate_resource("self:resources/icons/grocery.svg") row.name.text: "Groceries"}
            work := ListRowBtn{
                row.icon.draw_icon.color: mod.reminders.work
                row.icon.draw_icon.svg: crate_resource("self:resources/icons/briefcase.svg")
                row.name.text: "Work"
            }
            home := ListRowBtn{
                row.icon.draw_icon.color: mod.reminders.home
                row.icon.draw_icon.svg: crate_resource("self:resources/icons/house.svg")
                row.name.text: "Home"
            }
            travel := ListRowBtn{
                row.icon.draw_icon.color: mod.reminders.travel
                row.icon.draw_icon.svg: crate_resource("self:resources/icons/suitcase.svg")
                row.name.text: "Travel"
            }
        }
    }

    let ListContents = SolidView{
        width: Fill height: Fill flow: Down
        draw_bg +: {color: theme.color_bg_app}
        toolbar := View{
            width: Fill height: 56 flow: Right padding: Inset{left: 8 right: 32} align: Align{y: 0.5}
            back := ButtonFlat{
                height: 44 width: 72 margin: 0 text: "Back"
                icon_walk: Walk{width: 16 height: 16}
                draw_icon +: {svg: crate_resource("self:resources/icons/back.svg") color: theme.color_focus}
                draw_text +: {color: theme.color_focus}
            }
            short_title := Label{visible: false width: Fill max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.reminders.ink text_style: theme.font_bold{font_size: 24}}}
            spacer := View{width: Fill}
            new := glass.GlassButton{height: 44 text: "New Reminder"}
        }
        heading := View{
            width: Fill height: 40 flow: Right padding: Inset{left: 32 right: 32}
            title := Label{width: Fill draw_text +: {color: mod.reminders.ink text_style: theme.font_bold{font_size: 32}}}
            count := Label{padding: 0 draw_text +: {color: mod.reminders.ink_secondary text_style: theme.font_regular{font_size: 15}}}
        }
        completed_summary := ButtonFlat{
            height: 44 margin: Inset{left: 32 right: 32} text: "0 completed · Show"
            draw_text +: {color: mod.reminders.ink_secondary}
            draw_bg +: {color: #0000 border_size: 0.0}
        }
        rows := PortalList{
            width: Fill height: Fill
            padding: Inset{left: 32 right: 32}
            Section := Label{
                width: Fill height: 28
                draw_text +: {color: mod.reminders.ink_secondary text_style: theme.font_regular{font_size: 13}}
            }
            Item := ReminderRow{}
            CompletedSection := ButtonFlat{width: Fill height: 44 text: "" draw_text +: {color: mod.reminders.ink_secondary} draw_bg +: {color: #0000 border_size: 0.0}}
            Empty := Label{
                width: Fill height: 80
                align: Align{x: 0.5 y: 0.5}
                draw_text +: {color: mod.reminders.ink_secondary}
            }
        }
        composer := Composer{margin: Inset{left: 32 right: 32 bottom: 12}}
    }

    mod.reminders.ListPickerItem = PopupMenuItem{
        width: Fill height: #(crate::engine::LIST_PICKER_ITEM_HEIGHT)
        padding: Inset{left: 16 right: 16}
    }
    mod.reminders.ListPicker = DropDown{
        width: Fill height: 44 margin: 0
        popup_menu +: {menu_item: mod.reminders.ListPickerItem{}}
        labels: ["Groceries", "Work", "Home", "Travel"]
    }

    let DetailContents = SolidView{
        width: Fill height: Fill flow: Down
        draw_bg +: {color: theme.color_bg_container}
        header := View{
            width: Fill height: 56 flow: Right padding: Inset{left: 8 right: 8} align: Align{y: 0.5}
            cancel := ButtonFlat{height: 44 margin: 0 text: "Cancel" draw_text +: {color: theme.color_focus}}
            heading := Label{width: Fill align: Align{x: 0.5 y: 0.5} text: "Details" draw_text +: {color: mod.reminders.ink text_style: theme.font_bold{font_size: 16}}}
            done := glass.GlassButton{height: 44 text: "Done"}
        }
        fields := ScrollYView{
            width: Fill height: Fill flow: Down
            padding: 16 spacing: 20
            text_group := RoundedView{
                width: Fill height: Fit flow: Down
                draw_bg +: {color: mod.reminders.card border_radius: 12}
                title := TextInputFlat{width: Fill height: 64 is_multiline: true empty_text: "Title" margin: 0}
                notes := TextInputFlat{width: Fill height: 104 is_multiline: true submit_on_enter: false empty_text: "Notes" margin: 0}
            }
            schedule := RoundedView{
                width: Fill height: Fit flow: Down padding: 8 spacing: 4
                draw_bg +: {color: mod.reminders.card border_radius: 12}
                date_on := CheckBox{height: 52 text: "Date"}
                date := TextInputFlat{height: 44 empty_text: "YYYY-MM-DD" margin: 0}
                presets := View{
                    width: Fill height: 44 flow: Right spacing: 8
                    preset_today := ButtonFlat{height: 44 margin: 0 text: "Today"}
                    preset_tomorrow := ButtonFlat{height: 44 margin: 0 text: "Tomorrow"}
                    preset_week := ButtonFlat{height: 44 margin: 0 text: "In 1 Week"}
                }
                time_on := CheckBox{height: 52 text: "Time"}
                time := TextInputFlat{height: 44 empty_text: "HH:MM" margin: 0}
            }
            organization := RoundedView{
                width: Fill height: Fit flow: Down padding: 8 spacing: 8
                draw_bg +: {color: mod.reminders.card border_radius: 12}
                list := mod.reminders.ListPicker{}
                flag := CheckBox{height: 52 text: "Flag"}
                priority := glass.GlassSegmented{
                    width: Fill height: 44
                    labels: ["None", "Low", "Medium", "High"]
                }
            }
            error := Label{visible: false draw_text +: {color: theme.color_error wrap: Words}}
            delete := ButtonFlat{height: 44 margin: 0 text: "Delete Reminder" draw_text +: {color: theme.color_error}}
            confirm := View{
                visible: false width: Fill height: 44 flow: Right spacing: 8
                delete_cancel := ButtonFlat{height: 44 width: Fill margin: 0 text: "Cancel"}
                delete_confirm := ButtonFlat{height: 44 width: Fill margin: 0 text: "Delete" draw_text +: {color: theme.color_error}}
            }
            footer := Label{draw_text +: {color: mod.reminders.ink_secondary text_style: theme.font_regular{font_size: 12}}}
        }
    }

    mod.widgets.RemindersViewBase = #(RemindersView::register_widget(vm))
    mod.widgets.RemindersView = set_type_default() do mod.widgets.RemindersViewBase{
        width: Fill height: Fill
        flow: Overlay
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}

        theme_ink: mod.reminders.ink
        theme_ink_secondary: mod.reminders.ink_secondary
        theme_selected: mod.reminders.selected
        theme_today: mod.reminders.today
        theme_scheduled: mod.reminders.scheduled
        theme_flagged: mod.reminders.flagged
        theme_groceries: mod.reminders.groceries
        theme_home: mod.reminders.home
        theme_travel: mod.reminders.travel
        theme_all: mod.reminders.all
        theme_completed: mod.reminders.completed

        loading_shell := SolidView{
            visible: true width: Fill height: Fill
            draw_bg +: {color: theme.color_bg_app}
            align: Align{x: 0.5 y: 0.5}
            loading := Label{text: "Loading reminders…" draw_text +: {color: theme.color_text_meta}}
        }

        wide := View{
            visible: false width: Fill height: Fill flow: Overlay
            columns := View{
                width: Fill height: Fill flow: Right
                sidebar := HomeContents{
                    width: 280
                    show_bg: true
                    draw_bg +: {color: theme.color_bg_container}
                }
                list := ListContents{width: Fill}
            }
            overlay := View{
                visible: false width: Fill height: Fill flow: Overlay
                align: Align{x: 0.5 y: 0.5}
                scrim := Hit{}
                popover := GlassPanel{
                    width: 360 height: 640
                    padding: 0 spacing: 0
                    draw_bg +: {corner_radius: 18}
                    form := DetailContents{}
                }
            }
        }

        compact := View{
            visible: false width: Fill height: Fill flow: Overlay
            nav := StackNavigation{
                root_view := View{
                    width: Fill height: Fill flow: Overlay
                    home := HomeContents{}
                    fab := glass.GlassButton{
                        width: 56 height: 56
                        abs_pos: vec2(326, 708)
                        text: ""
                        icon_walk: Walk{width: 22 height: 22}
                        draw_icon +: {svg: crate_resource("self:resources/icons/plus.svg") color: theme.color_text}
                    }
                }
                list_view := StackNavigationView{
                    header.visible: false
                    body +: {margin: 0 list := ListContents{}}
                }
                detail_view := StackNavigationView{
                    header.visible: false
                    body +: {margin: 0 form := DetailContents{}}
                }
            }
        }

        save_strip := View{
            visible: false width: Fill height: 60 flow: Right padding: 8 spacing: 8
            show_bg: true
            draw_bg.color: theme.color_error
            save_msg := Label{width: Fill text: "Could not save reminders" draw_text +: {color: theme.color_text_on_accent}}
            retry := ButtonFlat{height: 44 margin: 0 text: "Retry" draw_text +: {color: theme.color_text_on_accent}}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct RemindersView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[live]
    theme_ink: Vec4,
    #[live]
    theme_ink_secondary: Vec4,
    #[live]
    theme_selected: Vec4,
    #[live]
    theme_today: Vec4,
    #[live]
    theme_scheduled: Vec4,
    #[live]
    theme_flagged: Vec4,
    #[live]
    theme_groceries: Vec4,
    #[live]
    theme_home: Vec4,
    #[live]
    theme_travel: Vec4,
    #[live]
    theme_all: Vec4,
    #[live]
    theme_completed: Vec4,
    #[rust]
    started: bool,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    machine: StorageMachine,
    #[rust]
    get_id: Option<StorageRequestId>,
    #[rust]
    list_id: Option<StorageRequestId>,
    #[rust]
    set_id: Option<StorageRequestId>,
    #[rust]
    document: Option<Document>,
    #[rust]
    now: Now,
    #[rust]
    utc_fallback: bool,
    #[rust]
    nav: NavState,
    #[rust]
    visual_route: Route,
    #[rust]
    draft: Option<Draft>,
    #[rust]
    draft_fields: Option<DraftFields>,
    #[rust]
    draft_seeded: bool,
    #[rust]
    confirm_delete: bool,
    #[rust]
    field_error: Option<String>,
    #[rust]
    projection: Option<Projection>,
    #[rust]
    list_rows: Vec<ListRow>,
    #[rust]
    tick: Option<Timer>,
    #[rust]
    last_size: Vec2d,
    #[rust]
    queued_nav: bool,
}

impl RemindersView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    pub fn ai_summary(&self) -> String {
        match &self.projection {
            Some(projection) => format!(
                "Reminders · {} open · Today {}",
                projection.counts.all, projection.counts.today
            ),
            None => "Reminders are loading".into(),
        }
    }

    pub fn ai_answer(&self, call: &ServiceCall) -> ToolResult {
        crate::ai::answer(
            self.document.as_ref(),
            self.machine.saved_revision,
            self.now,
            call,
        )
    }

    pub fn shutdown(&mut self, cx: &mut Cx) {
        if let Some(timer) = self.tick.take() {
            cx.stop_timer(timer);
        }
    }

    #[cfg(test)]
    pub(crate) fn timer_is_active(&self) -> bool {
        self.tick.is_some()
    }

    fn sample_now(&mut self) {
        let epoch = Cx::time_now().max(0.0) as i64;
        let offset = script_local_utc_offset_secs();
        self.now = Now::from_epoch_secs(epoch, offset);
        self.utc_fallback = offset == 0;
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.sample_now();
        self.tick = Some(cx.start_interval(MINUTE_TICK_SECS));
        if let Some(storage) = self.storage.clone() {
            match self.machine.start() {
                StorageAction::Get => self.get_id = Some(storage.get(cx, "document.json")),
                StorageAction::List => {
                    self.list_id = Some(storage.list(cx, "", None, 32));
                }
                StorageAction::None => {}
            }
        }
        self.rebuild(cx);
    }

    fn rebuild(&mut self, cx: &mut Cx) {
        if let Some(doc) = &self.document {
            let projection = project(doc, self.nav.filter, self.now);
            self.list_rows = flatten_rows(&projection, self.nav.filter, doc.show_completed);
            self.projection = Some(projection);
        } else {
            self.projection = None;
            self.list_rows.clear();
        }
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn ready(&self) -> bool {
        self.document.is_some() && self.machine.phase == StoragePhase::Ready
    }

    fn persist(&mut self, cx: &mut Cx) {
        let Some(doc) = &self.document else { return };
        self.machine.mark_dirty(doc.revision);
        if !self.machine.wants_save() {
            self.sync_chrome(cx);
            return;
        }
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let Ok(bytes) = encode(doc) else {
            self.machine.error = Some("Could not save reminders");
            self.sync_chrome(cx);
            return;
        };
        let id = storage.set(cx, "document.json", bytes);
        self.set_id = Some(id);
        self.machine.begin_save(id.0, doc.revision);
        self.sync_chrome(cx);
    }

    fn mutate(&mut self, cx: &mut Cx, command: Command) -> bool {
        if !self.ready() {
            return false;
        }
        let Some(doc) = self.document.as_mut() else {
            return false;
        };
        match apply(doc, command, self.now) {
            Ok(()) => {
                self.rebuild(cx);
                self.persist(cx);
                true
            }
            Err(CommandError::NoOp) => false,
            Err(_) => false,
        }
    }

    fn open_filter(&mut self, cx: &mut Cx, filter: Filter) {
        self.nav.open_list(filter);
        if let Some(mut list) = self
            .view
            .widget(cx, ids!(wide.list.rows))
            .borrow_mut::<PortalList>()
        {
            list.set_first_id_and_scroll(0, 0.0);
        }
        if let Some(mut list) = self
            .view
            .widget(cx, ids!(compact.nav.list_view.list.rows))
            .borrow_mut::<PortalList>()
        {
            list.set_first_id_and_scroll(0, 0.0);
        }
        self.sync_compact_nav(cx);
        self.rebuild(cx);
    }

    fn open_draft(&mut self, cx: &mut Cx, draft: Draft, selected: Option<ReminderId>) {
        self.draft_fields = Some(DraftFields::from_draft(&draft));
        self.draft = Some(draft);
        self.draft_seeded = false;
        self.confirm_delete = false;
        self.field_error = None;
        self.nav.open_detail(selected);
        self.sync_compact_nav(cx);
        self.rebuild(cx);
    }

    fn discard_draft(&mut self, cx: &mut Cx) {
        self.draft = None;
        self.draft_fields = None;
        self.draft_seeded = false;
        self.confirm_delete = false;
        self.field_error = None;
        self.nav.close_detail();
        self.sync_compact_nav(cx);
        self.rebuild(cx);
    }

    fn sync_compact_nav(&mut self, cx: &mut Cx) {
        if !self.nav.layout.is_compact() {
            return;
        }
        let nav = self.view.stack_navigation(cx, ids!(compact.nav));
        if nav.is_transitioning() {
            self.queued_nav = true;
            return;
        }
        let want = self.nav.current();
        let visual = match nav.current_view() {
            Some(id) if id == live_id!(detail_view) => Route::Detail,
            Some(_) => Route::List(self.nav.filter),
            None => Route::Home,
        };
        self.visual_route = visual;
        match compact_nav_step(visual, want, self.nav.filter) {
            None => self.visual_route = want,
            Some((CompactNavOp::PopToRoot, next)) => {
                nav.pop_to_root(cx);
                self.visual_route = next;
            }
            Some((CompactNavOp::PushList, next)) => {
                nav.push(cx, live_id!(list_view));
                self.visual_route = next;
                if next != want {
                    self.queued_nav = true;
                }
            }
            Some((CompactNavOp::PushDetail, next)) => {
                nav.push(cx, live_id!(detail_view));
                self.visual_route = next;
            }
            Some((CompactNavOp::PopToList, next)) => {
                nav.pop_to_view(cx, live_id!(list_view));
                self.visual_route = next;
            }
        }
    }

    fn home_parent<'a>(&'a self, cx: &'a mut Cx, compact: bool) -> WidgetRef {
        if compact {
            self.view.widget(cx, ids!(compact.nav.root_view.home))
        } else {
            self.view.widget(cx, ids!(wide.columns.sidebar))
        }
    }

    fn list_parent(&self, cx: &mut Cx, compact: bool) -> WidgetRef {
        if compact {
            self.view.widget(cx, ids!(compact.nav.list_view.list))
        } else {
            self.view.widget(cx, ids!(wide.columns.list))
        }
    }

    fn form_parent(&self, cx: &mut Cx, compact: bool) -> WidgetRef {
        if compact {
            self.view.widget(cx, ids!(compact.nav.detail_view.form))
        } else {
            self.view.widget(cx, ids!(wide.overlay.popover.form))
        }
    }

    fn sync_chrome(&mut self, cx: &mut Cx) {
        self.refresh_theme(cx);
        let loading = self.document.is_none();
        self.view
            .view(cx, ids!(loading_shell))
            .set_visible(cx, loading);
        let compact = self.nav.layout.is_compact();
        self.view
            .view(cx, ids!(wide))
            .set_visible(cx, !loading && !compact);
        self.view
            .view(cx, ids!(compact))
            .set_visible(cx, !loading && compact);
        let save_err = self.machine.error.is_some() && self.document.is_some();
        self.view
            .view(cx, ids!(save_strip))
            .set_visible(cx, save_err);
        self.view
            .view(cx, ids!(wide.overlay))
            .set_visible(cx, !compact && self.draft.is_some());
        if loading {
            return;
        }
        let Some(doc) = self.document.clone() else {
            return;
        };
        let Some(projection) = self.projection.clone() else {
            return;
        };
        self.bind_home(cx, false, &doc, &projection);
        self.bind_home(cx, true, &doc, &projection);
        self.bind_list(cx, false, &doc, &projection);
        self.bind_list(cx, true, &doc, &projection);
        if self.draft.is_some() {
            self.bind_detail(cx, compact);
        }
    }

    fn label_height(cx: &mut Cx, label: &LabelRef, text: &str) -> f64 {
        label
            .borrow()
            .map(|label| {
                label
                    .draw_text
                    .layout(cx, 0.0, 0.0, None, false, Align::default(), text)
                    .size_in_lpxs
                    .height as f64
            })
            .unwrap_or(0.0)
    }

    fn home_geometry(&self, cx: &mut Cx, home: &WidgetRef, compact: bool) {
        let short = !compact && self.nav.layout.is_short();
        let mode = if compact {
            LayoutMode::Compact
        } else if short {
            LayoutMode::WideShort
        } else {
            LayoutMode::Wide
        };
        let inset = if compact { 20.0 } else { 16.0 };
        let top = if compact { 16.0 } else { 12.0 };
        let mut home_view = home.clone();
        apply_owned!(cx, self.source, home_view, {padding: Inset{left: #(inset) right: #(inset) top: #(top) bottom: 88}});
        let heading = home.label(cx, ids!(heading));
        if let Some(mut label) = heading.borrow_mut() {
            label.draw_text.text_style.font_size = if compact { 34.0 } else { 24.0 };
        }
        let gap = if compact { 12.0 } else { 10.0 };
        for (path, first) in [(ids!(first), true), (ids!(second), false)] {
            let mut row = home.view(cx, path);
            let margin = if first {
                if compact {
                    20.0
                } else {
                    16.0
                }
            } else {
                gap
            };
            apply_owned!(cx, self.source, row, {spacing: #(gap) margin: Inset{top: #(margin)}});
        }
        let tile_height = smart_tile_height(mode);
        let padding = if compact { 14.0 } else { 8.0 };
        let top_height: f64 = if compact {
            38.0
        } else if short {
            32.0
        } else {
            36.0
        };
        let count_size = if compact {
            28.0
        } else if short {
            22.0
        } else {
            26.0
        };
        let name_size = if compact { 15.0 } else { 13.0 };
        for path in [ids!(today), ids!(scheduled), ids!(all), ids!(flagged)] {
            let mut tile = home.widget(cx, path);
            apply_owned!(cx, self.source, tile, { height: #(tile_height) });
            // Eval applies to a View do not propagate to its child widgets.
            // Set each child's metrics directly, then fit the theme's font to
            // the count line box before computing the remaining tile padding.
            let count = tile.label(cx, ids!(body.top.count));
            let name = tile.label(cx, ids!(body.name));
            if let Some(mut label) = name.borrow_mut() {
                label.draw_text.text_style.font_size = name_size;
            }
            let name_height = Self::label_height(cx, &name, "Scheduled");
            if let Some(mut label) = count.borrow_mut() {
                label.draw_text.text_style.font_size = count_size;
            }
            let count_height = Self::label_height(cx, &count, "2000");
            let max_count_height = top_height.min((tile_height - name_height - 4.0).max(22.0));
            if count_height > max_count_height {
                if let Some(mut label) = count.borrow_mut() {
                    label.draw_text.text_style.font_size *=
                        (max_count_height / count_height) as f32;
                }
            }
            let top_height = Self::label_height(cx, &count, "2000").max(22.0);
            let vertical_padding = ((tile_height - top_height - name_height) * 0.5).max(0.0);
            let body = tile.view(cx, ids!(body));
            if let Some(mut body) = body.borrow_mut() {
                body.layout.padding = Inset {
                    left: padding,
                    right: padding,
                    top: vertical_padding,
                    bottom: vertical_padding,
                };
            }
            let top = tile.view(cx, ids!(body.top));
            if let Some(mut top) = top.borrow_mut() {
                top.walk.height = Size::Fixed(top_height);
            }
            if let Some(mut label) = name.borrow_mut() {
                label.walk.height = Size::Fixed(name_height);
            }
            let _ = {
                if let Some(mut inner) = tile.borrow_mut::<View>() {
                    inner.walk.height = Size::Fixed(tile_height);
                }
            };
        }
        let mut completed = home.widget(cx, ids!(completed));
        let completed_height = completed_tile_height(mode);
        apply_owned!(cx, self.source, completed, {
            height: #(completed_height) margin: Inset{top: #(gap)}
        });
        let _ = {
            if let Some(mut inner) = completed.borrow_mut::<View>() {
                inner.walk.height = Size::Fixed(completed_height);
            }
        };
        for (path, size) in [
            (ids!(body.top.count), count_size),
            (ids!(body.top.name), name_size),
        ] {
            let label = completed.label(cx, path);
            if let Some(mut label) = label.borrow_mut() {
                label.draw_text.text_style.font_size = size;
            };
        }
        let mut my_lists = home.label(cx, ids!(my_lists));
        let my_margin = if compact { 24.0 } else { 20.0 };
        apply_owned!(cx, self.source, my_lists, {margin: Inset{top: #(my_margin)}});
        if let Some(mut label) = my_lists.borrow_mut() {
            label.draw_text.text_style.font_size = name_size;
        }
        let mut lists = home.view(cx, ids!(lists));
        let list_gap = if compact { 12.0 } else { 8.0 };
        apply_owned!(cx, self.source, lists, {margin: Inset{top: #(list_gap)}});
        for path in [
            ids!(lists.groceries),
            ids!(lists.work),
            ids!(lists.home),
            ids!(lists.travel),
        ] {
            let mut row = home.widget(cx, path);
            let height = personal_row_height(mode);
            apply_owned!(cx, self.source, row, {height: #(height)});
            for path in [ids!(row.name), ids!(row.count)] {
                let label = row.label(cx, path);
                if let Some(mut label) = label.borrow_mut() {
                    label.draw_text.text_style.font_size = if compact { 17.0 } else { 14.0 };
                };
            }
        }
    }

    fn list_geometry(&self, cx: &mut Cx, list: &WidgetRef, compact: bool) {
        let short = !compact && self.nav.layout.is_short();
        let inset = if compact {
            20.0
        } else if short {
            16.0
        } else {
            32.0
        };
        let mut heading = list.view(cx, ids!(heading));
        let font = if compact { 34.0 } else { 32.0 };
        let height = if compact { 42.0 } else { 40.0 };
        let top = if compact { 12.0 } else { 16.0 };
        apply_owned!(cx, self.source, heading, {
            visible: #(!short) height: #(height) margin: Inset{top: #(top)}
            padding: Inset{left: #(inset) right: #(inset)}
        });
        let title = list.label(cx, ids!(heading.title));
        if let Some(mut title) = title.borrow_mut() {
            title.draw_text.text_style.font_size = font;
        }
        list.label(cx, ids!(toolbar.short_title))
            .set_visible(cx, short);
        list.view(cx, ids!(toolbar.spacer)).set_visible(cx, !short);
        let mut toolbar = list.view(cx, ids!(toolbar));
        apply_owned!(cx, self.source, toolbar, {padding: Inset{left: 8 right: #(inset)}});
        let mut summary = list.button(cx, ids!(completed_summary));
        apply_owned!(cx, self.source, summary, {margin: Inset{left: #(inset) right: #(inset)}});
        let mut rows = list.portal_list(cx, ids!(rows));
        apply_owned!(cx, self.source, rows, {padding: Inset{left: #(inset) right: #(inset)}});
        let mut composer = list.view(cx, ids!(composer));
        let bottom = if short { 8.0 } else { 12.0 };
        apply_owned!(cx, self.source, composer, {margin: Inset{left: #(inset) right: #(inset) top: 8 bottom: #(bottom)}});
    }

    fn detail_geometry(&self, cx: &mut Cx, form: &WidgetRef, compact: bool) {
        let font = if compact { 17.0 } else { 14.0 };
        let mut header = form.view(cx, ids!(header));
        let height = if compact { 56.0 } else { 52.0 };
        apply_owned!(cx, self.source, header, {height: #(height)});
        for path in [
            ids!(fields.text_group.title),
            ids!(fields.text_group.notes),
            ids!(fields.schedule.date),
            ids!(fields.schedule.time),
        ] {
            let mut field = form.text_input(cx, path);
            apply_owned!(cx, self.source, field, {draw_text: {text_style: {font_size: #(font)}}});
        }
        for path in [
            ids!(fields.schedule.date_on),
            ids!(fields.schedule.time_on),
            ids!(fields.organization.flag),
        ] {
            let mut field = form.check_box(cx, path);
            apply_owned!(cx, self.source, field, {draw_text: {text_style: {font_size: #(font)}}});
        }
        let mut picker = form.drop_down(cx, ids!(fields.organization.list));
        apply_owned!(cx, self.source, picker, {draw_text: {text_style: {font_size: #(font)}}});
    }

    fn bind_home(&mut self, cx: &mut Cx, compact: bool, doc: &Document, projection: &Projection) {
        let home = self.home_parent(cx, compact);
        self.home_geometry(cx, &home, compact);
        let counts = projection.counts;
        let set = |cx: &mut Cx,
                   home: &WidgetRef,
                   id: &[LiveId],
                   count: usize,
                   selected: bool,
                   fill: Vec4| {
            home.widget(cx, id)
                .label(cx, ids!(body.top.count))
                .set_text(cx, &count.to_string());
            let mut tile = home.widget(cx, id);
            let border = if selected { 2.0 } else { 0.0 };
            let outline = self.theme_today;
            // Hosted roots may own a separate VM; keep this patch in that VM.
            apply_owned!(cx, self.source, tile, { draw_bg: {color: #(fill) border_size: #(border) border_color: #(outline)} });
        };
        let today = self.theme_color("today");
        let scheduled = self.theme_color("scheduled");
        let all = self.theme_color("all");
        let flagged = self.theme_color("flagged");
        let completed = self.theme_color("completed");
        set(
            cx,
            &home,
            ids!(today),
            counts.today,
            self.nav.filter == Filter::Today,
            today,
        );
        set(
            cx,
            &home,
            ids!(scheduled),
            counts.scheduled,
            self.nav.filter == Filter::Scheduled,
            scheduled,
        );
        set(
            cx,
            &home,
            ids!(all),
            counts.all,
            self.nav.filter == Filter::All,
            all,
        );
        set(
            cx,
            &home,
            ids!(flagged),
            counts.flagged,
            self.nav.filter == Filter::Flagged,
            flagged,
        );
        set(
            cx,
            &home,
            ids!(completed),
            counts.completed,
            self.nav.filter == Filter::Completed,
            completed,
        );
        let personal = [
            (ids!(lists.groceries), GROCERIES_LIST_ID, counts.groceries),
            (ids!(lists.work), WORK_LIST_ID, counts.work),
            (ids!(lists.home), HOME_LIST_ID, counts.home),
            (ids!(lists.travel), TRAVEL_LIST_ID, counts.travel),
        ];
        for (id, list_id, count) in personal {
            let row = home.widget(cx, id);
            row.label(cx, ids!(row.count))
                .set_text(cx, &count.to_string());
            if let Some(list) = doc.list(list_id) {
                row.label(cx, ids!(row.name)).set_text(cx, &list.name);
            }
            let selected = self.nav.filter == Filter::List(list_id);
            let mut face = row.clone();
            let highlight = if selected {
                self.theme_selected
            } else {
                vec4(0.0, 0.0, 0.0, 0.0)
            };
            apply_owned!(cx, self.source, face, { draw_bg: {color: #(highlight)} });
        }
    }

    fn refresh_theme(&mut self, cx: &mut Cx) {
        let Some(vm_id) = cx.script_ref_vm_id(&self.source) else {
            return;
        };
        cx.with_script_vm_id(vm_id, |vm| {
            let theme = vm.module(id!(theme));
            let mut read = |key: LiveId, fallback: Vec4| {
                let value = vm.bx.heap.value(theme, key.into(), NoTrap);
                if value.is_nil() || value.is_err() {
                    fallback
                } else {
                    Vec4::script_from_value(vm, value)
                }
            };
            self.theme_ink = read(id!(color_text), self.theme_ink);
            let secondary = read(id!(color_text_meta), self.theme_ink_secondary);
            self.theme_ink_secondary = read(id!(color_text_muted), secondary);
            self.theme_today = read(id!(color_focus), self.theme_today);
            self.theme_scheduled = read(id!(color_error), self.theme_scheduled);
            self.theme_flagged = read(id!(color_warning), self.theme_flagged);
            self.theme_home = self.theme_flagged;
            self.theme_groceries = read(id!(color_success), self.theme_groceries);
            self.theme_selected = read(id!(color_bg_highlight), self.theme_selected);
            let background = read(id!(color_bg_container), self.theme_all);
            self.theme_travel = self.theme_today * 0.55 + self.theme_scheduled * 0.45;
            self.theme_all = background * 0.78 + self.theme_ink * 0.22;
            self.theme_completed = background * 0.62 + self.theme_ink * 0.38;
        });
    }

    fn theme_color(&self, name: &str) -> Vec4 {
        match name {
            "ink" => self.theme_ink,
            "ink_secondary" => self.theme_ink_secondary,
            "selected" => self.theme_selected,
            "today" => self.theme_today,
            "scheduled" => self.theme_scheduled,
            "flagged" => self.theme_flagged,
            "groceries" => self.theme_groceries,
            "home" => self.theme_home,
            "travel" => self.theme_travel,
            "all" => self.theme_all,
            "completed" => self.theme_completed,
            _ => self.theme_today,
        }
    }

    fn bind_list(&mut self, cx: &mut Cx, compact: bool, doc: &Document, projection: &Projection) {
        let list = self.list_parent(cx, compact);
        self.list_geometry(cx, &list, compact);
        list.label(cx, ids!(heading.title))
            .set_text(cx, &projection.title);
        list.label(cx, ids!(toolbar.short_title))
            .set_text(cx, &projection.title);
        let colour = match self.nav.filter {
            Filter::Today | Filter::List(WORK_LIST_ID) => self.theme_today,
            Filter::Scheduled => self.theme_scheduled,
            Filter::Flagged | Filter::List(HOME_LIST_ID) => self.theme_flagged,
            Filter::List(GROCERIES_LIST_ID) => self.theme_groceries,
            Filter::List(TRAVEL_LIST_ID) => self.theme_travel,
            _ => self.theme_ink,
        };
        list.label(cx, ids!(heading.title))
            .set_text_color(cx, colour);
        list.label(cx, ids!(toolbar.short_title))
            .set_text_color(cx, colour);
        list.label(cx, ids!(heading.count))
            .set_text(cx, &projection.active_count.to_string());
        let summary = if projection.completed_count == 0 {
            String::new()
        } else if doc.show_completed {
            format!("{} completed · Hide", projection.completed_count)
        } else {
            format!("{} completed · Show", projection.completed_count)
        };
        list.button(cx, ids!(completed_summary))
            .set_text(cx, &summary);
        list.button(cx, ids!(completed_summary)).set_visible(
            cx,
            projection.completed_count > 0 && !matches!(self.nav.filter, Filter::Completed),
        );
        list.view(cx, ids!(composer))
            .set_visible(cx, self.nav.filter.allows_create());
        list.widget(cx, ids!(toolbar.new))
            .set_visible(cx, self.nav.filter.allows_create());
        list.button(cx, ids!(toolbar.back)).set_visible(cx, compact);
        list.view(cx, ids!(toolbar)).set_visible(cx, true);
    }

    fn bind_detail(&mut self, cx: &mut Cx, compact: bool) {
        let Some(draft) = self.draft.clone() else {
            return;
        };
        let form = self.form_parent(cx, compact);
        self.detail_geometry(cx, &form, compact);
        let Some(fields) = self.draft_fields.clone() else {
            return;
        };
        if !self.draft_seeded {
            form.text_input(cx, ids!(fields.text_group.title))
                .set_text(cx, &fields.title);
            form.text_input(cx, ids!(fields.text_group.notes))
                .set_text(cx, &fields.notes);
            form.check_box(cx, ids!(fields.schedule.date_on))
                .set_active(cx, fields.date_on, Animate::No);
            form.text_input(cx, ids!(fields.schedule.date))
                .set_text(cx, &fields.date);
            form.check_box(cx, ids!(fields.schedule.time_on))
                .set_active(cx, fields.time_on, Animate::No);
            form.text_input(cx, ids!(fields.schedule.time))
                .set_text(cx, &fields.time);
            form.check_box(cx, ids!(fields.organization.flag))
                .set_active(cx, fields.flagged, Animate::No);
            if let Some(mut seg) = form
                .widget(cx, ids!(fields.organization.priority))
                .borrow_mut::<GlassSegmented>()
            {
                seg.set_selected(cx, fields.priority.index());
            }
            if let Some(doc) = &self.document {
                let drop = form.drop_down(cx, ids!(fields.organization.list));
                drop.set_labels(cx, doc.lists.iter().map(|list| list.name.clone()).collect());
                if let Some(index) = doc.lists.iter().position(|list| list.id == fields.list_id) {
                    drop.set_selected_item(cx, index);
                }
            }
        }
        form.widget(cx, ids!(fields.schedule.date))
            .set_visible(cx, fields.date_on);
        form.view(cx, ids!(fields.schedule.presets))
            .set_visible(cx, fields.date_on);
        form.widget(cx, ids!(fields.schedule.time))
            .set_visible(cx, fields.date_on && fields.time_on);
        let err = self.field_error.clone();
        form.label(cx, ids!(fields.error))
            .set_visible(cx, err.is_some());
        if let Some(err) = err {
            form.label(cx, ids!(fields.error)).set_text(cx, &err);
        }
        form.button(cx, ids!(fields.delete))
            .set_visible(cx, draft.original_id.is_some() && !self.confirm_delete);
        form.view(cx, ids!(fields.confirm))
            .set_visible(cx, draft.original_id.is_some() && self.confirm_delete);
        let footer = if self.utc_fallback {
            "Dates shown in UTC"
        } else {
            ""
        };
        form.label(cx, ids!(fields.footer)).set_text(cx, footer);
        self.draft_seeded = true;
    }

    fn capture_form(&mut self, cx: &mut Cx) {
        if !self.draft_seeded {
            return;
        }
        let form = self.form_parent(cx, self.nav.layout.is_compact());
        let Some(fields) = self.draft_fields.as_mut() else {
            return;
        };
        fields.title = form.text_input(cx, ids!(fields.text_group.title)).text();
        fields.notes = form.text_input(cx, ids!(fields.text_group.notes)).text();
        fields.date_on = form.check_box(cx, ids!(fields.schedule.date_on)).active(cx);
        fields.time_on = form.check_box(cx, ids!(fields.schedule.time_on)).active(cx);
        fields.date = form.text_input(cx, ids!(fields.schedule.date)).text();
        fields.time = form.text_input(cx, ids!(fields.schedule.time)).text();
        fields.flagged = form
            .check_box(cx, ids!(fields.organization.flag))
            .active(cx);
        if let Some(seg) = form
            .widget(cx, ids!(fields.organization.priority))
            .borrow::<GlassSegmented>()
        {
            fields.priority = Priority::from_index(seg.selected());
        }
        let index = form
            .drop_down(cx, ids!(fields.organization.list))
            .selected_item();
        if let Some(list) = self.document.as_ref().and_then(|doc| doc.lists.get(index)) {
            fields.list_id = list.id;
        }
    }

    fn read_draft_from_form(&mut self, cx: &mut Cx) -> Result<Draft, String> {
        self.capture_form(cx);
        let draft = self.draft.clone().ok_or("No draft")?;
        self.draft_fields
            .as_ref()
            .ok_or("No draft")?
            .apply_to(draft)
            .map_err(str::to_string)
    }

    fn commit_draft(&mut self, cx: &mut Cx) {
        match self.read_draft_from_form(cx) {
            Ok(draft) => {
                match self
                    .document
                    .as_mut()
                    .and_then(|doc| Some(apply(doc, Command::ApplyDraft(draft), self.now)))
                {
                    Some(Ok(())) => {
                        self.field_error = None;
                        self.draft = None;
                        self.draft_fields = None;
                        self.draft_seeded = false;
                        self.confirm_delete = false;
                        self.nav.close_detail();
                        self.sync_compact_nav(cx);
                        self.rebuild(cx);
                        self.persist(cx);
                    }
                    Some(Err(CommandError::NoOp)) => {
                        self.field_error = None;
                        self.discard_draft(cx);
                    }
                    Some(Err(error)) => {
                        self.field_error = Some(
                            match error {
                                CommandError::Rejected(message) => message,
                                _ => "Could not save this reminder",
                            }
                            .into(),
                        );
                        self.sync_chrome(cx);
                    }
                    None => {}
                }
            }
            Err(err) => {
                self.field_error = Some(err);
                self.draft_seeded = true;
                self.sync_chrome(cx);
                self.view.redraw(cx);
            }
        }
    }

    fn create_from_composer(&mut self, cx: &mut Cx, text: &str) {
        let Some(defaults) = create_defaults(self.nav.filter, self.now) else {
            return;
        };
        if self.mutate(
            cx,
            Command::Create {
                title: text.to_string(),
                list_id: defaults.list_id,
                due: defaults.due,
                flagged: defaults.flagged,
            },
        ) {
            let list = self.list_parent(cx, self.nav.layout.is_compact());
            list.text_input(cx, ids!(composer.title)).set_text(cx, "");
            list.text_input(cx, ids!(composer.title)).take_key_focus(cx);
        }
    }

    fn handle_home_clicks(&mut self, cx: &mut Cx, actions: &Actions) {
        for compact in [false, true] {
            let home = self.home_parent(cx, compact);
            if home.button(cx, ids!(today.hit)).clicked(actions) {
                self.open_filter(cx, Filter::Today);
            }
            if home.button(cx, ids!(scheduled.hit)).clicked(actions) {
                self.open_filter(cx, Filter::Scheduled);
            }
            if home.button(cx, ids!(all.hit)).clicked(actions) {
                self.open_filter(cx, Filter::All);
            }
            if home.button(cx, ids!(flagged.hit)).clicked(actions) {
                self.open_filter(cx, Filter::Flagged);
            }
            if home.button(cx, ids!(completed.hit)).clicked(actions) {
                self.open_filter(cx, Filter::Completed);
            }
            if home.button(cx, ids!(lists.groceries.hit)).clicked(actions) {
                self.open_filter(cx, Filter::List(GROCERIES_LIST_ID));
            }
            if home.button(cx, ids!(lists.work.hit)).clicked(actions) {
                self.open_filter(cx, Filter::List(WORK_LIST_ID));
            }
            if home.button(cx, ids!(lists.home.hit)).clicked(actions) {
                self.open_filter(cx, Filter::List(HOME_LIST_ID));
            }
            if home.button(cx, ids!(lists.travel.hit)).clicked(actions) {
                self.open_filter(cx, Filter::List(TRAVEL_LIST_ID));
            }
        }
        if self
            .view
            .widget(cx, ids!(compact.nav.root_view.fab))
            .borrow::<GlassButton>()
            .is_some_and(|b| b.clicked(actions))
        {
            let defaults = home_create_defaults();
            self.open_draft(cx, blank_draft(&defaults), None);
        }
    }

    fn handle_list_clicks(&mut self, cx: &mut Cx, actions: &Actions) {
        for compact in [false, true] {
            let list = self.list_parent(cx, compact);
            if list.button(cx, ids!(toolbar.back)).clicked(actions) {
                self.nav.open_home();
                self.sync_compact_nav(cx);
                self.rebuild(cx);
            }
            if list
                .widget(cx, ids!(toolbar.new))
                .borrow::<GlassButton>()
                .is_some_and(|b| b.clicked(actions))
            {
                if let Some(defaults) = create_defaults(self.nav.filter, self.now) {
                    self.open_draft(cx, blank_draft(&defaults), None);
                }
            }
            if list.button(cx, ids!(completed_summary)).clicked(actions) {
                if let Some(doc) = &self.document {
                    let show = !doc.show_completed;
                    self.mutate(cx, Command::SetShowCompleted { show });
                }
            }
            if let Some((text, _)) = list.text_input(cx, ids!(composer.title)).returned(actions) {
                self.create_from_composer(cx, &text);
            }
            if list.button(cx, ids!(composer.add)).clicked(actions) {
                let text = list.text_input(cx, ids!(composer.title)).text();
                self.create_from_composer(cx, &text);
            }
            if let Some(mut portal) = list.widget(cx, ids!(rows)).borrow_mut::<PortalList>() {
                for (index, row) in self.list_rows.iter().enumerate() {
                    match row {
                        ListRow::Item(item) => {
                            let widget = portal.item(cx, index, live_id!(Item));
                            if widget.button(cx, ids!(complete.hit)).clicked(actions) {
                                let id = item.id;
                                let completed = !item.completed;
                                drop(portal);
                                self.mutate(cx, Command::SetCompleted { id, completed });
                                return;
                            }
                            if widget.button(cx, ids!(details)).clicked(actions) {
                                let id = item.id;
                                drop(portal);
                                if let Some(doc) = &self.document {
                                    if let Some(reminder) = doc.reminder(id) {
                                        let draft = crate::engine::draft_from(reminder);
                                        self.open_draft(cx, draft, Some(id));
                                    }
                                }
                                return;
                            }
                        }
                        ListRow::CompletedSummary { expanded, .. } => {
                            let widget = portal.item(cx, index, live_id!(CompletedSection));
                            if widget.button(cx, &[]).clicked(actions) {
                                let show = !*expanded;
                                drop(portal);
                                self.mutate(cx, Command::SetShowCompleted { show });
                                return;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn handle_detail_clicks(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.draft.is_none() {
            return;
        }
        if self
            .view
            .button(cx, ids!(wide.overlay.scrim))
            .clicked(actions)
        {
            self.discard_draft(cx);
            return;
        }
        for compact in [self.nav.layout.is_compact()] {
            let form = self.form_parent(cx, compact);
            if form.button(cx, ids!(header.cancel)).clicked(actions)
                || form
                    .text_input(cx, ids!(fields.text_group.title))
                    .escaped(actions)
                || form
                    .text_input(cx, ids!(fields.text_group.notes))
                    .escaped(actions)
            {
                self.discard_draft(cx);
                return;
            }
            if form
                .widget(cx, ids!(header.done))
                .borrow::<GlassButton>()
                .is_some_and(|b| b.clicked(actions))
            {
                self.commit_draft(cx);
                return;
            }
            if let Some((_, mods)) = form
                .text_input(cx, ids!(fields.text_group.notes))
                .returned(actions)
            {
                if mods.logo || mods.control {
                    self.commit_draft(cx);
                    return;
                }
            }
            if let Some((_, mods)) = form
                .text_input(cx, ids!(fields.text_group.title))
                .returned(actions)
            {
                if mods.logo || mods.control {
                    self.commit_draft(cx);
                    return;
                }
            }
            if let Some(on) = form
                .check_box(cx, ids!(fields.schedule.date_on))
                .changed(actions)
            {
                if let Some(fields) = self.draft_fields.as_mut() {
                    fields.set_date_enabled(on, self.now);
                }
                self.draft_seeded = false;
                self.sync_chrome(cx);
            }
            if let Some(on) = form
                .check_box(cx, ids!(fields.schedule.time_on))
                .changed(actions)
            {
                self.toggle_time(cx, on);
            }
            if form
                .button(cx, ids!(fields.schedule.presets.preset_today))
                .clicked(actions)
            {
                self.apply_preset(cx, 0);
            }
            if form
                .button(cx, ids!(fields.schedule.presets.preset_tomorrow))
                .clicked(actions)
            {
                self.apply_preset(cx, 1);
            }
            if form
                .button(cx, ids!(fields.schedule.presets.preset_week))
                .clicked(actions)
            {
                self.apply_preset(cx, 7);
            }
            if form.button(cx, ids!(fields.delete)).clicked(actions) {
                self.confirm_delete = true;
                self.sync_chrome(cx);
            }
            if form
                .button(cx, ids!(fields.confirm.delete_cancel))
                .clicked(actions)
            {
                self.confirm_delete = false;
                self.sync_chrome(cx);
            }
            if form
                .button(cx, ids!(fields.confirm.delete_confirm))
                .clicked(actions)
            {
                if let Some(id) = self.draft.as_ref().and_then(|d| d.original_id) {
                    self.discard_draft(cx);
                    self.mutate(cx, Command::Delete { id });
                    return;
                }
            }
        }
    }

    fn toggle_time(&mut self, cx: &mut Cx, on: bool) {
        self.capture_form(cx);
        if let Some(fields) = self.draft_fields.as_mut() {
            self.field_error = fields
                .set_time_enabled(on, self.now)
                .err()
                .map(str::to_string);
            if self.field_error.is_some() {
                fields.time_on = !on;
            }
        }
        self.draft_seeded = false;
        self.sync_chrome(cx);
    }

    fn apply_preset(&mut self, cx: &mut Cx, days: i32) {
        self.capture_form(cx);
        if let Some(fields) = self.draft_fields.as_mut() {
            fields.date_on = true;
            fields.date = makepad_civil_time::format_iso(self.now.day + days);
        }
        self.draft_seeded = false;
        self.sync_chrome(cx);
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self
                .storage
                .as_ref()
                .is_some_and(|storage| storage.namespace() != response.namespace)
            {
                continue;
            }
            if self.get_id == Some(response.request_id) && response.op == StorageOp::Get {
                let event = match &response.result {
                    Ok(StorageResult::Value(None)) => StorageEvent::GetMissing,
                    Ok(StorageResult::Value(Some(bytes))) if bytes.is_empty() => {
                        StorageEvent::GetEmpty
                    }
                    Ok(StorageResult::Value(Some(bytes))) => StorageEvent::GetBytes(bytes.clone()),
                    Err(_) => StorageEvent::GetFailed,
                    _ => continue,
                };
                self.get_id = None;
                if let Some(outcome) = self.machine.on_get(event) {
                    self.apply_load(cx, outcome);
                } else if self.machine.phase == StoragePhase::ListInFlight {
                    if let Some(storage) = self.storage.as_ref() {
                        self.list_id = Some(storage.list(cx, "", None, 32));
                    }
                }
            }
            if self.list_id == Some(response.request_id) && response.op == StorageOp::List {
                let event = match &response.result {
                    Ok(StorageResult::List(list))
                        if list.keys.is_empty() && list.next_cursor.is_none() =>
                    {
                        StorageEvent::ListEmpty
                    }
                    Ok(StorageResult::List(_)) => StorageEvent::ListNotEmpty,
                    Err(_) => StorageEvent::ListFailed,
                    _ => continue,
                };
                self.list_id = None;
                if let Some(outcome) = self.machine.on_list(event) {
                    self.apply_load(cx, outcome);
                }
            }
            if self.set_id == Some(response.request_id) && response.op == StorageOp::Set {
                let event = match &response.result {
                    Ok(StorageResult::Unit) => StorageEvent::SetOk {
                        request_id: response.request_id.0,
                    },
                    Err(_) => StorageEvent::SetFailed {
                        request_id: response.request_id.0,
                    },
                    _ => continue,
                };
                if self.machine.on_set(event) {
                    self.set_id = None;
                    if self.machine.wants_save() {
                        self.persist(cx);
                    } else {
                        self.sync_chrome(cx);
                    }
                }
            }
        }
    }

    fn apply_load(&mut self, cx: &mut Cx, outcome: LoadOutcome) {
        match outcome {
            LoadOutcome::Seed => {
                let doc = seed(self.now.day);
                self.machine.saved_revision = 0;
                self.machine.dirty_revision = doc.revision;
                self.document = Some(doc);
                self.persist(cx);
                self.rebuild(cx);
            }
            LoadOutcome::Loaded(doc) => {
                self.document = Some(doc);
                self.rebuild(cx);
            }
            LoadOutcome::Error(message) => {
                self.view
                    .label(cx, ids!(loading_shell.loading))
                    .set_text(cx, message);
                self.rebuild(cx);
            }
        }
    }

    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let layout = decide_layout(size.x, size.y, Some(self.nav.layout));
        if layout != self.nav.layout {
            self.capture_form(cx);
            let previous = self
                .list_parent(cx, self.nav.layout.is_compact())
                .portal_list(cx, ids!(rows));
            let anchor = previous
                .borrow()
                .map(|list| (list.first_id(), list.first_scroll()));
            self.nav.resize(layout);
            self.draft_seeded = false;
            if let Some((id, scroll)) = anchor {
                self.list_parent(cx, layout.is_compact())
                    .portal_list(cx, ids!(rows))
                    .set_first_id_and_scroll(id, scroll);
            }
            self.sync_compact_nav(cx);
            self.rebuild(cx);
        }
        if layout.is_wide() {
            let mut sidebar = self.view.view(cx, ids!(wide.columns.sidebar));
            let sidebar_width = if layout.is_short() { 236.0 } else { 280.0 };
            apply_owned!(cx, self.source, sidebar, { width: #(sidebar_width) });
            let mut pop = self.view.widget(cx, ids!(wide.overlay.popover));
            let width =
                (size.x - 32.0)
                    .max(44.0)
                    .min(if layout.is_short() { 560.0 } else { 360.0 });
            let height = (size.y - 32.0).max(44.0).min(640.0);
            let pos = dvec2((size.x - width) * 0.5, (size.y - height) * 0.5);
            apply_owned!(cx, self.source, pop, { width: #(width) height: #(height) abs_pos: #(pos) });
            let _ = {
                if let Some(mut inner) = pop.borrow_mut::<View>() {
                    inner.walk.width = Size::Fixed(width);
                    inner.walk.height = Size::Fixed(height);
                    inner.walk.abs_pos = Some(pos);
                }
            };
        }
        if layout.is_compact() {
            let mut fab = self.view.widget(cx, ids!(compact.nav.root_view.fab));
            let x = (size.x - 76.0).max(8.0);
            let y = (size.y - 72.0).max(8.0);
            apply_owned!(cx, self.source, fab, { abs_pos: #(dvec2(x, y)) });
        }
    }

    fn measure_row(
        &self,
        cx: &mut Cx,
        widget: &WidgetRef,
        item: &crate::engine::ProjectedItem,
        width: f64,
    ) -> f64 {
        let compact = self.nav.layout.is_compact();
        let text_width = (width - 44.0 - 44.0 - 8.0).max(1.0);
        let mut content_height = 0.0;
        let mut title_lines = 1;
        for (path, text, max_lines, font, line_height) in [
            (
                ids!(text.title),
                Some(item.title.as_str()),
                2,
                if compact { 17.0 } else { 15.0 },
                if compact { 22.0 } else { 20.0 },
            ),
            (
                ids!(text.notes),
                item.notes_preview.as_deref(),
                1,
                if compact { 13.0 } else { 12.0 },
                if compact { 18.0 } else { 17.0 },
            ),
            (
                ids!(text.metadata),
                item.metadata.as_deref(),
                if self.nav.layout.is_short() { 0 } else { 1 },
                if compact { 13.0 } else { 12.0 },
                if compact { 18.0 } else { 17.0 },
            ),
        ] {
            let label = widget.label(cx, path);
            if let Some(mut label) = label.borrow_mut() {
                label.draw_text.text_style.font_size = font;
                label.max_lines = max_lines;
                label.draw_text.max_lines = max_lines;
                label.draw_text.text_overflow = label.text_overflow;
                if let Some(text) = text {
                    let measured = label.draw_text.layout(
                        cx,
                        0.0,
                        0.0,
                        Some(text_width as f32),
                        true,
                        Align::default(),
                        text,
                    );
                    let lines = measured.rows.len().max(1);
                    if path == ids!(text.title) {
                        title_lines = lines as u32;
                    }
                    let height =
                        (measured.size_in_lpxs.height as f64).max(line_height * lines as f64);
                    label.walk.height = Size::Fixed(height);
                    content_height += height;
                }
            };
        }
        let padding = if compact { 10.0 } else { 8.0 };
        let mut text = widget.view(cx, ids!(text));
        apply_owned!(cx, self.source, text, {padding: Inset{top: #(padding) bottom: #(padding) right: 8}});
        let measured_height = ((content_height + padding * 2.0) / 4.0).ceil() * 4.0;
        measured_height.max(row_height(
            compact,
            title_lines,
            item.notes_preview.is_some(),
            item.metadata.is_some(),
        ))
    }

    fn draw_rows(&self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.list_rows.len());
        while let Some(index) = list.next_visible_item(cx) {
            if index >= self.list_rows.len() {
                continue;
            }
            match &self.list_rows[index] {
                ListRow::Section(text) => {
                    let item = list.item(cx, index, live_id!(Section));
                    item.set_text(cx, text);
                    item.draw_all(cx, &mut Scope::empty());
                }
                ListRow::Empty(text) => {
                    let item = list.item(cx, index, live_id!(Empty));
                    item.set_text(cx, text);
                    item.draw_all(cx, &mut Scope::empty());
                }
                ListRow::CompletedSummary { count, expanded } => {
                    let item = list.item(cx, index, live_id!(CompletedSection));
                    let label = if *expanded {
                        format!("{count} completed · Hide")
                    } else {
                        format!("{count} completed · Show")
                    };
                    item.set_text(cx, &label);
                    item.draw_all(cx, &mut Scope::empty());
                }
                ListRow::Item(item) => {
                    let widget = list.item(cx, index, live_id!(Item));
                    widget.label(cx, ids!(text.title)).set_text(cx, &item.title);
                    widget
                        .label(cx, ids!(text.notes))
                        .set_text(cx, item.notes_preview.as_deref().unwrap_or(""));
                    widget
                        .label(cx, ids!(text.notes))
                        .set_visible(cx, item.notes_preview.is_some());
                    widget
                        .label(cx, ids!(text.metadata))
                        .set_text(cx, item.metadata.as_deref().unwrap_or(""));
                    widget
                        .label(cx, ids!(text.metadata))
                        .set_visible(cx, item.metadata.is_some());
                    let width = cx.turtle().rect().size.x;
                    let height = self.measure_row(cx, &widget, item, width);
                    let mut row = widget.clone();
                    apply_owned!(cx, self.source, row, { height: #(height) });
                    let _ = {
                        if let Some(mut inner) = row.borrow_mut::<View>() {
                            inner.walk.height = Size::Fixed(height);
                        }
                    };
                    let ink = if item.completed {
                        self.theme_ink_secondary
                    } else {
                        self.theme_ink
                    };
                    let meta = if item.overdue && !item.completed {
                        self.theme_scheduled
                    } else {
                        self.theme_ink_secondary
                    };
                    let title = widget.label(cx, ids!(text.title));
                    title.set_text_color(cx, ink);
                    let notes = widget.label(cx, ids!(text.notes));
                    notes.set_text_color(cx, self.theme_ink_secondary);
                    let metadata = widget.label(cx, ids!(text.metadata));
                    metadata.set_text_color(cx, meta);
                    widget
                        .view(cx, ids!(complete.empty))
                        .set_visible(cx, !item.completed);
                    widget
                        .view(cx, ids!(complete.checked))
                        .set_visible(cx, item.completed);
                    widget.draw_all(cx, &mut Scope::empty());
                }
            }
        }
    }
}

impl Widget for RemindersView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if self
            .tick
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
            || matches!(event, Event::Foreground)
        {
            let previous = self.now;
            self.sample_now();
            if self.now != previous {
                self.rebuild(cx);
            }
        }
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::Escape && self.draft.is_some() {
                self.discard_draft(cx);
                return;
            }
        }
        if let Event::BackPressed { .. } = event {
            if event.back_pressed() {
                if self.draft.is_some() {
                    self.discard_draft(cx);
                    return;
                }
                if self.nav.layout.is_compact() && !matches!(self.nav.current(), Route::Home) {
                    self.nav.back();
                    self.sync_compact_nav(cx);
                    self.rebuild(cx);
                    return;
                }
            }
        }
        let overlay_up = self.draft.is_some() && self.nav.layout.is_wide();
        let input = matches!(
            event,
            Event::MouseDown(_)
                | Event::MouseUp(_)
                | Event::MouseMove(_)
                | Event::TouchUpdate(_)
                | Event::Scroll(_)
        );
        if overlay_up && input {
            self.view
                .widget(cx, ids!(wide.overlay))
                .handle_event(cx, event, scope);
        } else {
            self.view.handle_event(cx, event, scope);
        }
        self.capture_form(cx);
        if let Event::Actions(actions) = event {
            if self
                .view
                .button(cx, ids!(save_strip.retry))
                .clicked(actions)
            {
                if self.machine.retry_save() {
                    self.persist(cx);
                }
            }
            if self.ready() {
                self.handle_home_clicks(cx, actions);
                self.handle_list_clicks(cx, actions);
                self.handle_detail_clicks(cx, actions);
            }
            if self.queued_nav {
                self.queued_nav = false;
                self.sync_compact_nav(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        if size.x >= 1.0
            && ((size.x - self.last_size.x).abs() + (size.y - self.last_size.y).abs() > 0.5)
        {
            self.last_size = size;
            self.apply_layout(cx, size);
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_rows(cx, &mut list);
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod layout_tests {
    use crate::engine::{
        completed_tile_height, decide_layout, personal_row_height, smart_tile_height, LayoutMode,
        LIST_PICKER_ITEM_HEIGHT, MINUTE_TICK_SECS,
    };

    #[test]
    fn the_layout_follows_the_room_the_view_is_given() {
        assert_eq!(decide_layout(402.0, 780.0, None), LayoutMode::Compact);
        assert_eq!(decide_layout(699.0, 800.0, None), LayoutMode::Compact);
        assert_eq!(decide_layout(700.0, 800.0, None), LayoutMode::Wide);
        assert_eq!(decide_layout(1240.0, 800.0, None), LayoutMode::Wide);
        assert_eq!(decide_layout(874.0, 300.0, None), LayoutMode::WideShort);
        assert_eq!(
            decide_layout(f64::INFINITY, 800.0, Some(LayoutMode::Compact)),
            LayoutMode::Compact
        );
        assert_eq!(smart_tile_height(LayoutMode::Compact), 88.0);
        assert_eq!(completed_tile_height(LayoutMode::Wide), 52.0);
        assert_eq!(personal_row_height(LayoutMode::Compact), 56.0);
        assert_eq!(LIST_PICKER_ITEM_HEIGHT, 44.0);
        assert_eq!(MINUTE_TICK_SECS, 60.0);
    }
}

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
