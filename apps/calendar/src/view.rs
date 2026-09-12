//! The sole Calendar document, navigation, draft, clock, and storage owner.
use crate::agenda::{AgendaFrame, CalendarAgendaList};
use crate::ai::ToolAvailability;
use crate::components::*;
use crate::engine::*;
use crate::model::*;
use crate::month::{CalendarMiniMonth, CalendarMonthCanvas, MonthFrame};
use crate::navigation::{CalendarBodyDeck, CalendarOverlayHost, OverlayAction, OverlayKind};
use crate::presentation::*;
use crate::storage::{encode, load_bytes, LoadOutcome, Persist, PersistStatus, SaveAck, JAIL_KEY};
use crate::timeline::{CalendarTimeline, CalendarTimelineHeaders, TimelineFrame};
use makepad_civil_time::{self as civil, Day};
use makepad_widgets::makepad_platform::storage::{StorageHandle, StorageResponse, StorageResult};
use makepad_widgets::*;
use std::collections::HashSet;
macro_rules! path { ($($id:ident).+) => { &ids!($($id).+)[..] }; }

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.calendar.*
    use mod.shader.*

    mod.widgets.CalendarToolbar = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:56 flow:Right padding:Inset{left:16 right:16} spacing:8 align:Align{y:0.5}
        add := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/add.svg")}}
        mode := glass.GlassSegmented{width:240 height:44 labels:["Month","Week","Day"]}
        toolbar_space := Plain{width:Fill}
        search := QuietAction{icon_walk:Walk{width:0 height:0} draw_icon +: {color:action svg:crate_resource("self:resources/icons/search.svg")} width:220 text:"Search" align:Align{x:0.0 y:0.5} draw_text.color:secondary}
        previous := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/previous.svg")}}
        today := QuietAction{width:64 text:"Today"}
        next := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/next.svg")}}
    }
    mod.widgets.CalendarPeriodHeading = Plain{height:48 flow:Right padding:Inset{left:20 right:20} spacing:8 align:Align{y:0.5}
        period_title := Ink{height:Fill draw_text.text_style:theme.font_bold{font_size:21}}
        period_year := Ink{height:Fill draw_text.text_style:theme.font_regular{font_size:21}}
    }
    mod.widgets.CalendarSidebar = Plain{width:220 flow:Down show_bg:true draw_bg.color:sidebar
        sidebar_title := Ink{height:56 width:Fill padding:Inset{left:16} text:"Calendars" draw_text.text_style:theme.font_bold{font_size:12.75}}
        account_caption := Ink{height:28 width:Fill padding:Inset{left:16} text:"On This Device" draw_text +: {color:secondary text_style:theme.font_regular{font_size:8.25}}}
        calendar_rows := mod.widgets.CalendarRows{}
        sidebar_space := Plain{height:Fill}
        storage_status := mod.widgets.AppStorageStatus{}
        mini_month := mod.widgets.CalendarMiniMonth{height:208}
    }
    mod.widgets.CalendarPhoneNavigation = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:52 flow:Right padding:Inset{left:16 right:16} align:Align{y:0.5}
        year := QuietAction{width:72 text:"2026" draw_text +: {color:action text_style:theme.font_regular{font_size:12.75}}}
        previous := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/previous.svg")}}
        next := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/next.svg")}}
        navigation_space := Plain{width:Fill}
        view_menu := Plain{width:44 height:44 flow:Overlay
            icon := Icon{width:44 height:44 align:Align{x:0.5 y:0.5} icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/view-menu.svg")}}
            menu := QuietChoice{width:44 height:44 padding:0 labels:["Month","Month + List","Day","Agenda"] selected_item:1 popup_menu_position:#(makepad_widgets::drop_down::PopupMenuPosition::BelowInput)
                draw_text +: {get_color:fn(){return vec4(0.0,0.0,0.0,0.0)}}
                popup_menu +: {menu_item +: {height:44}}
            }
        }
        add := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/add.svg")}}
    }
    mod.widgets.CalendarPhoneTitle = Ink{height:52 width:Fill padding:Inset{left:20} draw_text.text_style:theme.font_bold{font_size:25.5}}
    mod.widgets.CalendarSelectedDate = Ink{height:36 width:Fill padding:Inset{left:20} draw_text.text_style:theme.font_bold{font_size:11.25}}
    mod.widgets.CalendarPhoneToolbar = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:64 padding:Inset{left:16 right:16 top:10 bottom:10}
        capsule := GlassPanel{draw_bg.border_radius:11.0 width:Fill height:44 flow:Right padding:Inset{left:12 right:12} spacing:0
            today := QuietAction{width:72 text:"Today" draw_text.color:action}
            left_space := Plain{width:Fill}
            calendars := QuietAction{width:88 text:"Calendars" draw_text.color:action}
            right_space := Plain{width:Fill}
            search := QuietAction{width:72 text:"Search" draw_text.color:action}
        }
    }
    mod.widgets.CalendarLandscapeToolbar = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:52 flow:Right padding:Inset{left:12 right:12} spacing:4 align:Align{y:0.5}
        calendars := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/calendars.svg")}}
        period := Ink{width:150 height:Fill draw_text.text_style:theme.font_bold{font_size:11.25}}
        previous := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/previous.svg")}}
        today := QuietAction{width:60 text:"Today"}
        next := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/next.svg")}}
        mode := glass.GlassSegmented{width:180 height:44 labels:["Month","Week","Day"]}
        toolbar_space := Plain{width:Fill}
        search := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/search.svg")}}
        add := QuietAction{text:"" icon_walk:Walk{width:20 height:20} draw_icon +: {color:action svg:crate_resource("self:resources/icons/add.svg")}}
    }
    mod.widgets.CalendarEventDetail = ScrollYView{flow:Down padding:20 spacing:8 show_bg:true draw_bg.color:paper
        event_title := Ink{width:Fill height:Fit max_lines:0 flow:Right{wrap:true} draw_text.text_style:theme.font_bold{font_size:18}}
        occurrence_dates := Ink{width:Fill height:Fit max_lines:0 flow:Right{wrap:true} draw_text.text_style:theme.font_regular{font_size:11.25}}
        detail_gap := Plain{height:16}
        calendar_row := Ink{width:Fill height:44 draw_text.text_style:theme.font_regular{font_size:11.25}}
        repeat_row := Ink{width:Fill height:44 draw_text.text_style:theme.font_regular{font_size:11.25}}
        event_notes := Ink{width:Fill height:Fit margin:Inset{top:16 bottom:24} max_lines:0 flow:Right{wrap:true} draw_text.text_style:theme.font_regular{font_size:12.75 line_spacing:1.47}}
        delete_event := QuietAction{width:Fill height:52 text:"Delete Event" align:Align{x:0.0 y:0.5} draw_text.color:action}
    }
    let Group = mod.widgets.CalendarControlGroup{show_bg:true width:Fill height:Fit flow:Down padding:Inset{left:12 right:12} spacing:0 draw_bg +: {color:instance(chrome) border_radius:instance(6.0) pixel:fn(){
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,self.border_radius)
            sdf.fill(self.color)
            return sdf.result
        }}}
    mod.widgets.CalendarTitleGroup = Group{
        title_input := QuietField{height:64 empty_text:"Title"}
        title_error := Ink{width:Fill height:Fit visible:false max_lines:0 flow:Right{wrap:true} draw_text.color:action}
        title_rule := mod.widgets.AppRule{}
        calendar_choice := QuietChoice{height:52 labels:["Home","Work","Birthdays","Holidays"]}
    }
    let DateValue = Plain{width:Fill height:44 flow:Overlay
        date_input := QuietField{empty_text:"YYYY-MM-DD"}
        date_summary := Ink{width:Fill height:Fill draw_text.text_style:theme.font_regular{font_size:11.25}}
    }
    let TimingRow = Plain{height:Fit flow:Down
        values := Plain{height:52 flow:Right spacing:8 align:Align{y:0.5}
            field_label := Ink{width:52 text:"Starts" draw_text.text_style:theme.font_regular{font_size:11.25}}
            date_value := DateValue{}
            time_input := QuietField{width:68 empty_text:"HH:MM"}
        }
        field_error := Ink{width:Fill height:Fit visible:false max_lines:0 flow:Right{wrap:true} draw_text.color:action}
    }
    mod.widgets.CalendarTimingGroup = Group{
        all_day_row := Plain{height:52 flow:Right align:Align{y:0.5}
            field_label := Ink{width:Fill text:"All-day" draw_text.text_style:theme.font_regular{font_size:11.25}}
            all_day := Toggle{width:52 height:44 text:"" margin:0}
        }
        all_day_rule := mod.widgets.AppRule{}
        starts_row := TimingRow{}
        timing_rule := mod.widgets.AppRule{}
        ends_row := TimingRow{values +: {field_label.text:"Ends"}}
    }
    mod.widgets.CalendarRepeatGroup = Group{repeat_choice := QuietChoice{height:52 labels:["Does not repeat","Daily","Weekly","Monthly","Yearly"]}}
    mod.widgets.CalendarNotesGroup = Group{notes_input := QuietField{height:128 is_multiline:true submit_on_enter:false empty_text:"Notes"}}
    mod.widgets.CalendarValidation = Ink{width:Fill height:Fit visible:false max_lines:0 flow:Right{wrap:true} draw_text.color:action}
    mod.widgets.CalendarSeriesNotice = Ink{width:Fill height:Fit visible:false max_lines:0 flow:Right{wrap:true} text:"Changes apply to every occurrence." draw_text.color:secondary}
    mod.widgets.CalendarEditorHeader = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:56 flow:Right padding:Inset{left:12 right:12} align:Align{y:0.5}
        cancel := QuietAction{width:72 text:"Cancel" draw_text +: {color:action text_style:theme.font_regular{font_size:12.75}}}
        editor_title := Ink{width:Fill height:Fill align:Align{x:0.5 y:0.5} text:"New Event" draw_text.text_style:theme.font_bold{font_size:12.75}}
        save := QuietAction{width:72 text:"Save" draw_text +: {color:action text_style:theme.font_bold{font_size:12.75}}}
    }
    let Panel = GlassPanel{draw_bg.border_radius:8.0 width:Fill height:Fill padding:0 flow:Down
        surface := RoundedView{width:Fill height:Fill flow:Down padding:0 spacing:0 draw_bg +: {color:paper border_radius:8.0}}
    }
    mod.widgets.CalendarEditorPanel = Panel{surface +: {
        editor_header := mod.widgets.CalendarEditorHeader{}
        editor_scroll := mod.widgets.CalendarEditorScroll{flow:Down padding:16 spacing:16 show_bg:false
            title_group := mod.widgets.CalendarTitleGroup{}
            timing_group := mod.widgets.CalendarTimingGroup{}
            repeat_group := mod.widgets.CalendarRepeatGroup{}
            notes_group := mod.widgets.CalendarNotesGroup{}
            validation := mod.widgets.CalendarValidation{}
            series_notice := mod.widgets.CalendarSeriesNotice{}
        }
    }}
    mod.widgets.CalendarDetailHeader = mod.widgets.CalendarControlGroup{show_bg:true draw_bg.color:chrome height:52 flow:Right padding:Inset{left:12 right:12} align:Align{y:0.5}
        close := QuietAction{width:72 text:"Close" draw_text.color:action}
        header_space := Plain{width:Fill}
        edit := QuietAction{width:100 text:"Edit" draw_text.color:action}
    }
    mod.widgets.CalendarDetailPanel = Panel{surface +: {detail_header := mod.widgets.CalendarDetailHeader{} detail_content := mod.widgets.CalendarEventDetail{}}}
    mod.widgets.CalendarSearchField = Plain{height:56 padding:Inset{left:20 right:20} align:Align{y:0.5}
        query := QuietField{empty_text:"Search events"}
    }
    mod.widgets.CalendarSearchRange = Ink{width:Fill height:32 padding:Inset{left:20 right:20} draw_text +: {color:secondary text_style:theme.font_regular{font_size:8.25}}}
    mod.widgets.CalendarSearchPanel = Panel{surface +: {
        search_header := mod.widgets.CalendarDetailHeader{edit.visible:false}
        search_query := mod.widgets.CalendarSearchField{height:52}
        search_range := mod.widgets.CalendarSearchRange{}
        search_results := mod.widgets.CalendarAgendaList{}
    }}
    mod.widgets.CalendarCalendarsPanel = Panel{surface +: {
        calendars_header := mod.widgets.CalendarDetailHeader{edit.visible:false}
        calendar_scroll := ScrollYView{flow:Down show_bg:false padding:0
            calendar_choices := mod.widgets.CalendarRows{}
        }
    }}
    mod.widgets.AppConfirmation = Panel{surface +: {padding:16 spacing:12
        message := Ink{width:Fill height:Fit max_lines:0 flow:Right{wrap:true} draw_text.text_style:theme.font_bold{font_size:12.75}}
        cancel := QuietAction{height:44 width:Fill text:"Cancel"}
        confirm := QuietAction{height:44 width:Fill text:"Delete" draw_text.color:action}
    }}
    mod.widgets.CalendarViewBase = #(CalendarView::register_widget(vm))
    mod.widgets.CalendarView = set_type_default() do mod.widgets.CalendarViewBase{
        width:Fill height:Fill flow:Overlay show_bg:true draw_bg.color:paper
        wide := Plain{flow:Right
            sidebar := mod.widgets.CalendarSidebar{width:220}
            sidebar_rule := mod.widgets.AppRule{width:1 height:Fill vertical:true}
            main := Plain{flow:Down
                toolbar := mod.widgets.CalendarToolbar{height:56}
                period := mod.widgets.CalendarPeriodHeading{height:48}
                body := mod.widgets.CalendarBodyDeck{
                    month := mod.widgets.CalendarMonthPage{}
                    week := mod.widgets.CalendarTimeline{}
                    day := mod.widgets.CalendarTimeline{}
                    agenda := mod.widgets.CalendarAgendaPage{}
                }
            }
        }
        compact_host := mod.widgets.CalendarBodyDeck{visible:false active:@compact
        compact := StackNavigation{
            root_view +: {show_bg:true draw_bg.color:paper flow:Down padding:0 margin:0 spacing:0
                navigation := mod.widgets.CalendarPhoneNavigation{height:52}
                month_title := mod.widgets.CalendarPhoneTitle{height:52}
                weekdays := mod.widgets.CalendarWeekdays{height:24 inset:16 abbreviated:true}
                month_dates := mod.widgets.CalendarPhoneMonth{height:264 margin:Inset{left:16 right:16}}
                selected_date := mod.widgets.CalendarSelectedDate{height:36}
                selected_agenda := mod.widgets.CalendarAgendaList{}
                bottom_toolbar := mod.widgets.CalendarPhoneToolbar{height:64}
            }
            day_page := mod.widgets.CalendarStackPage{body +: {
                date_strip := mod.widgets.CalendarDateStrip{height:64}
                day_timeline := mod.widgets.CalendarTimeline{show_headers:false}
                day_toolbar := mod.widgets.CalendarPhoneToolbar{height:64}
            }}
            agenda_page := mod.widgets.CalendarStackPage{body +: {
                agenda_heading := mod.widgets.CalendarAgendaHeading{height:52}
                agenda_range := mod.widgets.CalendarSearchRange{}
                upcoming_agenda := mod.widgets.CalendarAgendaList{}
                agenda_toolbar := mod.widgets.CalendarPhoneToolbar{height:64}
            }}
            detail_page := mod.widgets.CalendarStackPage{header +: {content +: {right_container +: {header_action +: {text:"Edit"}}}}
                body +: {event_detail := mod.widgets.CalendarEventDetail{}}
            }
            calendars_page := mod.widgets.CalendarStackPage{body +: {
                calendar_scroll := ScrollYView{flow:Down show_bg:false padding:0
                    calendar_choices := mod.widgets.CalendarRows{}
                }
            }}
            search_page := mod.widgets.CalendarStackPage{body +: {
                search_query := mod.widgets.CalendarSearchField{height:56}
                search_range := mod.widgets.CalendarSearchRange{height:32}
                search_results := mod.widgets.CalendarAgendaList{}
            }}
        }
        }
        landscape := Plain{visible:false flow:Down
            short_toolbar := mod.widgets.CalendarLandscapeToolbar{height:52}
            short_body := mod.widgets.CalendarBodyDeck{active:@week
                week := mod.widgets.CalendarTimeline{}
                day := mod.widgets.CalendarTimeline{}
                month := mod.widgets.CalendarShortMonth{}
                agenda := mod.widgets.CalendarAgendaPage{}
            }
        }
        overlays := mod.widgets.CalendarOverlayHost{
            detail_panel := mod.widgets.CalendarDetailPanel{}
            editor_panel := mod.widgets.CalendarEditorPanel{}
            search_panel := mod.widgets.CalendarSearchPanel{}
            calendars_panel := mod.widgets.CalendarCalendarsPanel{}
            confirmation := mod.widgets.AppConfirmation{}
            agenda_panel := Panel{surface +: {
                agenda_header := mod.widgets.CalendarDetailHeader{edit.visible:false}
                agenda_date := mod.widgets.CalendarSelectedDate{}
                day_agenda := mod.widgets.CalendarAgendaList{}
            }}
        }
        storage_notice := RoundedView{visible:false width:320 height:44 padding:0 draw_bg +: {color:chrome border_radius:6.0}
            storage_status := mod.widgets.AppStorageStatus{height:44}
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PhoneMode {
    Month,
    #[default]
    MonthList,
    Day,
    Agenda,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Confirm {
    #[default]
    None,
    Discard,
    Delete(EventId),
}
#[derive(Script, ScriptHook, Widget)]
pub struct CalendarView {
    #[deref]
    view: View,
    #[rust]
    started: bool,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    persist: Persist,
    #[rust]
    document: Option<CalendarDocument>,
    #[rust]
    clock: ClockSnapshot,
    #[rust]
    layout: LayoutDecision,
    #[rust]
    dimensions: Vec2d,
    #[rust]
    selected: Day,
    #[rust]
    displayed: Day,
    #[rust]
    month_anchor_dom: u32,
    #[rust]
    mode: CalendarMode,
    #[rust]
    portrait_mode: CalendarMode,
    #[rust]
    phone_mode: PhoneMode,
    #[rust]
    root_phone_mode: PhoneMode,
    #[rust]
    query_editor: WidgetRef,
    #[rust]
    pages: Vec<LiveId>,
    #[rust]
    stack_transition: Option<(LiveId, bool)>,
    #[rust]
    selected_occ: Option<OccurrenceKey>,
    #[rust]
    draft: Option<EventDraft>,
    #[rust]
    fields: Option<DraftFields>,
    #[rust]
    original_fields: Option<DraftFields>,
    #[rust]
    errors: DraftErrors,
    #[rust]
    search_query: String,
    #[rust]
    confirm: Confirm,
    #[rust]
    confirm_return: OverlayKind,
    #[rust]
    tick: Option<Timer>,
    #[rust]
    saved_timer: Option<Timer>,
    #[rust]
    last_anchor: Rect,
    #[rust]
    theme: CalendarColors,
    #[rust]
    theme_initialized: bool,
    #[rust]
    recipe_theme: CalendarColors,
    #[rust]
    styled: HashSet<WidgetUid>,
    #[rust]
    week_scroll: Option<f64>,
    #[rust]
    day_scroll: Option<f64>,
    #[rust]
    agenda_scroll: f64,
    #[rust]
    agenda_upcoming: bool,
    #[rust]
    keyboard_height: f64,
    #[rust]
    keyboard_motion: KeyboardMotion,
    #[rust]
    selected_scroll: f64,
    #[rust]
    search_scroll: f64,
    #[live(false)]
    reduced_motion: bool,
}
struct KeyboardMotion {
    from: f64,
    target: f64,
    start: f64,
    duration: f64,
    ease: makepad_widgets::makepad_platform::event::Ease,
    next: NextFrame,
}
impl Default for KeyboardMotion {
    fn default() -> Self {
        Self {
            from: 0.0,
            target: 0.0,
            start: 0.0,
            duration: 0.0,
            ease: makepad_widgets::makepad_platform::event::Ease::Linear,
            next: NextFrame::default(),
        }
    }
}
fn toolbar_widths(main_width: f64) -> (f64, f64) {
    // 32 inset + six gaps + four navigation/actions = 276 points.
    if main_width < 560.0 { (180.0, 44.0) }
    else if main_width < 736.0 { (240.0, 44.0) }
    else { (240.0, 220.0) }
}
fn overlay_page(kind: OverlayKind) -> Option<(LiveId, &'static str)> {
    match kind {
        OverlayKind::Detail => Some((live_id!(detail_page), "Event")),
        OverlayKind::Search => Some((live_id!(search_page), "Search")),
        OverlayKind::Calendars => Some((live_id!(calendars_page), "Calendars")),
        OverlayKind::Agenda => Some((live_id!(agenda_page), "Agenda")),
        _ => None,
    }
}
fn descendants(view: &View) -> Vec<WidgetRef> {
    let mut all: Vec<_> = view.children.iter().map(|(_, w)| w.clone()).collect();
    let mut i = 0;
    while i < all.len() {
        let w = all[i].clone();
        w.try_children(&mut |_, child| all.push(child));
        i += 1;
    }
    all
}
impl CalendarView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }
    pub fn document(&self) -> Option<&CalendarDocument> {
        self.document.as_ref()
    }
    pub fn clock(&self) -> ClockSnapshot {
        self.clock
    }
    pub fn tool_availability(&self) -> ToolAvailability {
        match self.persist.status {
            PersistStatus::Loading => {
                ToolAvailability::Unavailable("the calendar is still loading")
            }
            PersistStatus::LoadError => {
                ToolAvailability::Unavailable("the calendar failed to load")
            }
            _ if self.document.is_none() => {
                ToolAvailability::Unavailable("the calendar is still loading")
            }
            _ => ToolAvailability::Ready,
        }
    }
    pub fn ai_summary(&self) -> String {
        let Some(doc) = &self.document else {
            return "Calendar is loading".into();
        };
        format!(
            "Calendar · {} · {} events today",
            civil::format_iso(self.clock.today),
            occurrences_for_day(doc, self.clock.today, false, 8)
                .items
                .len()
        )
    }
    pub fn seed_for_test(&mut self, today: Day) {
        self.clock = ClockSnapshot { today, minute: 540 };
        self.selected = today;
        self.displayed = civil::month_start(today);
        self.month_anchor_dom = civil::to_ymd(today).2;
        self.document = Some(crate::seed::seed(today));
        self.persist.status = PersistStatus::Ready;
    }
    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.clock = ClockSnapshot::from_epoch_secs(Cx::time_now() as i64);
        self.selected = self.clock.today;
        self.displayed = civil::month_start(self.selected);
        self.month_anchor_dom = civil::to_ymd(self.selected).2;
        if let Some(storage) = &self.storage {
            self.persist.begin_load(storage.get(cx, JAIL_KEY).0);
        } else {
            self.persist.status = PersistStatus::LoadError;
        }
        self.tick = Some(cx.start_interval(60.0));
        self.view.redraw(cx);
    }
    fn overlay(&self, cx: &mut Cx) -> WidgetRef {
        self.view.widget(cx, path!(overlays))
    }
    fn panel(&self, cx: &mut Cx) -> WidgetRef {
        self.view.widget(cx, path!(editor_panel))
    }
    fn current_overlay(&self, cx: &mut Cx) -> OverlayKind {
        self.overlay(cx)
            .borrow::<CalendarOverlayHost>()
            .map(|h| h.active)
            .unwrap_or_default()
    }
    fn show_overlay(&mut self, cx: &mut Cx, kind: OverlayKind) {
        if let Some(mut host) = self.overlay(cx).borrow_mut::<CalendarOverlayHost>() {
            host.show(cx, kind, self.last_anchor);
        }
        cx.set_key_focus(Area::Empty);
        self.view.redraw(cx);
    }
    fn close_overlay(&mut self, cx: &mut Cx) {
        let kind = self.current_overlay(cx);
        if self.layout.kind != LayoutKind::Compact
            && overlay_page(kind).is_some_and(|(id, _)| self.pages.last() == Some(&id))
        {
            self.back(cx);
        }
        if let Some(mut host) = self.overlay(cx).borrow_mut::<CalendarOverlayHost>() {
            host.close(cx);
        }
        cx.set_key_focus(Area::Empty);
        self.view.redraw(cx);
    }
    fn stack(&self, cx: &mut Cx) -> StackNavigationRef {
        self.view.stack_navigation(cx, path!(compact))
    }
    fn settle_stack(&mut self, cx: &mut Cx) {
        let stack = self.stack(cx);
        if !stack.is_transitioning() {
            return;
        }
        // Preserve the currently visible composite before completing the stock transition.
        // Its controller rejects a second navigation while transitioning; complete its
        // bookkeeping synchronously so the new input can be accepted immediately.
        if let Some(mut deck) = self
            .view
            .widget(cx, path!(compact_host))
            .borrow_mut::<CalendarBodyDeck>()
        {
            deck.transition(cx, 0);
        }
        let Some((id, push)) = self.stack_transition else {
            return;
        };
        let parent = stack.widget_uid();
        let uid = stack.view_by_id(cx, id).widget_uid();
        let actions = cx.capture_actions(|cx| {
            if push {
                cx.widget_action(uid, StackNavigationTransitionAction::ShowDone);
            } else {
                cx.widget_action(uid, StackNavigationTransitionAction::HideEnd(parent));
            }
        });
        stack.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
    }
    fn push_page(&mut self, cx: &mut Cx, id: LiveId, title: &str) {
        if self.pages.last() == Some(&id) {
            return;
        }
        self.settle_stack(cx);
        self.configure_stack_motion(cx);
        if self.reduced_motion {
            if let Some(mut deck) = self
                .view
                .widget(cx, path!(compact_host))
                .borrow_mut::<CalendarBodyDeck>()
            {
                deck.transition(cx, 0);
            }
        }
        let stack = self.stack(cx);
        stack.set_title(cx, id, title);
        stack.push(cx, id);
        self.stack_transition = Some((id, true));
        if self.reduced_motion {
            stack
                .view_by_id(cx, id)
                .as_stack_navigation_view()
                .show(cx, 0.0);
        }
        self.pages.push(id);
        self.view.redraw(cx);
    }
    fn back(&mut self, cx: &mut Cx) {
        self.settle_stack(cx);
        if self.reduced_motion {
            if let Some(mut deck) = self
                .view
                .widget(cx, path!(compact_host))
                .borrow_mut::<CalendarBodyDeck>()
            {
                deck.transition(cx, 0);
            }
        }
        let leaving = self.pages.pop();
        self.stack_transition = leaving.map(|id| (id, false));
        let stack = self.stack(cx);
        if self.reduced_motion {
            if let Some(id) = leaving {
                stack.view_by_id(cx, id).set_visible(cx, false);
            }
        }
        if let Some(id) = self.pages.last().copied() {
            stack.pop_to_view(cx, id);
        } else {
            stack.pop_to_root(cx);
            self.phone_mode = self.root_phone_mode;
            self.sync_modes(cx);
        }
        cx.set_key_focus(Area::Empty);
        self.view.redraw(cx);
    }
    fn configure_stack_motion(&self, cx: &mut Cx) {
        // Stock StackNavigationView closes after offset > width. The epsilon is subpixel.
        let width = self.dimensions.x.max(1.0) + 0.01;
        let push = if self.reduced_motion { 0.0 } else { 0.24 };
        let pop = if self.reduced_motion { 0.0 } else { 0.20 };
        for id in [
            live_id!(day_page),
            live_id!(agenda_page),
            live_id!(detail_page),
            live_id!(calendars_page),
            live_id!(search_page),
        ] {
            let mut page = self.stack(cx).view_by_id(cx, id);
            script_apply_eval!(cx,page,{animator +: {slide +: {hide +: {from:{all:mod.prelude.widgets_internal.Play.Forward{duration:#(pop)}} apply:{offset:#(width)}} show +: {from:{all:mod.prelude.widgets_internal.Play.Forward{duration:#(push)}}}}}});
        }
    }
    fn remember_scroll(&mut self, cx: &mut Cx) {
        let branch = if self.layout.kind == LayoutKind::Compact {
            path!(compact)
        } else if self.layout.short_height {
            path!(landscape)
        } else {
            path!(wide)
        };
        let parent = self.view.widget(cx, branch);
        let day = if self.layout.kind == LayoutKind::Compact {
            parent.widget(cx, path!(day_timeline))
        } else {
            parent.widget(cx, path!(day))
        };
        if let Some(t) = day.borrow::<CalendarTimeline>() {
            if let Some(y) = t.scroll_state(cx) {
                self.day_scroll = Some(y);
            }
        }
        if let Some(t) = parent.widget(cx, path!(week)).borrow::<CalendarTimeline>() {
            if let Some(y) = t.scroll_state(cx) {
                self.week_scroll = Some(y);
            }
        }
        if let Some(a) = parent
            .widget(cx, path!(upcoming_agenda))
            .borrow::<CalendarAgendaList>()
        {
            self.agenda_scroll = a.scroll();
        }
        if let Some(a) = self
            .view
            .widget(cx, path!(selected_agenda))
            .borrow::<CalendarAgendaList>()
        {
            self.selected_scroll = a.scroll();
        }
        if self.current_overlay(cx) == OverlayKind::Agenda {
            if let Some(a) = self
                .view
                .widget(cx, path!(day_agenda))
                .borrow::<CalendarAgendaList>()
            {
                if self.agenda_upcoming {
                    self.agenda_scroll = a.scroll();
                } else {
                    self.selected_scroll = a.scroll();
                }
            }
        }
        let search = if self.layout.kind == LayoutKind::Compact {
            path!(search_page.search_results)
        } else {
            path!(search_panel.search_results)
        };
        if let Some(a) = self.view.widget(cx, search).borrow::<CalendarAgendaList>() {
            self.search_scroll = a.scroll();
        }
    }
    fn apply_layout(&mut self, cx: &mut Cx, next: LayoutDecision, size: Vec2d) {
        self.remember_scroll(cx);
        self.settle_stack(cx);
        let was_compact = self.layout.kind == LayoutKind::Compact;
        let previous_overlay = self.current_overlay(cx);
        if next.short_height && !self.layout.short_height {
            self.portrait_mode = self.mode;
            if was_compact && self.phone_mode == PhoneMode::Day {
                self.mode = CalendarMode::Day;
            } else if self.mode == CalendarMode::Month {
                self.mode = CalendarMode::Week;
            }
        }
        if !next.short_height && self.layout.short_height {
            self.mode = self.portrait_mode;
        }
        self.layout = next;
        self.dimensions = size;
        let compact = next.kind == LayoutKind::Compact;
        let short = next.short_height && !compact;
        self.view
            .widget(cx, path!(wide))
            .set_visible(cx, !compact && !short);
        self.view
            .widget(cx, path!(landscape))
            .set_visible(cx, short);
        self.view
            .widget(cx, path!(compact_host))
            .set_visible(cx, compact);
        let mut sidebar = self.view.widget(cx, path!(wide.sidebar));
        let sidebar_width = next.sidebar_width();
        script_apply_eval!(cx,sidebar,{width:#(sidebar_width)});
        let main_width = size.x - sidebar_width - 1.0;
        let (mode_width, search_width) = toolbar_widths(main_width);
        let mut modes = self.view.widget(cx, path!(wide.toolbar.mode));
        script_apply_eval!(cx,modes,{width:#(mode_width)});
        let narrow_search = search_width < 100.0;
        let mut search = self.view.widget(cx, path!(wide.toolbar.search));
        script_apply_eval!(cx,search,{width:#(search_width)});
        search
            .as_button()
            .set_text(cx, if narrow_search { "" } else { "Search" });
        if narrow_search {
            script_apply_eval!(cx,search,{icon_walk +: {width:20 height:20}});
        } else {
            script_apply_eval!(cx,search,{icon_walk +: {width:0 height:0}});
        }
        if let Some(mut h) = self.overlay(cx).borrow_mut::<CalendarOverlayHost>() {
            h.compact = compact;
            h.short = short;
            h.keyboard_occlusion = self.keyboard_height;
        }
        let radius = if short {
            0.0
        } else if compact {
            10.0
        } else {
            8.0
        };
        let mut surface = self.panel(cx).widget(cx, path!(surface));
        script_apply_eval!(cx,surface,{draw_bg.border_radius:#(radius)});
        let mut header = self.panel(cx).widget(cx, path!(editor_header));
        let header_height = if compact || short { 52.0 } else { 56.0 };
        script_apply_eval!(cx,header,{height:#(header_height)});
        for widget in descendants(&self.view) {
            if let Some(mut deck) = widget.borrow_mut::<CalendarBodyDeck>() {
                deck.cut(cx);
            }
            if let Some(timeline) = widget.borrow::<CalendarTimeline>() {
                // The component keeps scroll while drawing; transfer it only across allocations.
                if timeline.frame.days.len() == 7 {
                    if let Some(y) = self.week_scroll {
                        timeline.set_scroll_y(cx, y);
                    }
                } else if let Some(y) = self.day_scroll {
                    timeline.set_scroll_y(cx, y);
                }
            }
        }
        if compact != was_compact
            && !matches!(
                previous_overlay,
                OverlayKind::Editor | OverlayKind::Confirmation
            )
        {
            if compact {
                if let Some((id, title)) = overlay_page(previous_overlay) {
                    if let Some(mut host) = self.overlay(cx).borrow_mut::<CalendarOverlayHost>() {
                        host.hide_immediately(cx);
                    }
                    self.push_page(cx, id, title);
                }
            } else if let Some(id) = self.pages.last().copied() {
                let kind = if id == live_id!(detail_page) {
                    OverlayKind::Detail
                } else if id == live_id!(search_page) {
                    OverlayKind::Search
                } else if id == live_id!(calendars_page) {
                    OverlayKind::Calendars
                } else if id == live_id!(agenda_page) {
                    OverlayKind::Agenda
                } else {
                    OverlayKind::None
                };
                if kind != OverlayKind::None {
                    self.show_overlay(cx, kind);
                }
            }
        }
        self.sync_search_fields(cx);
        for (paths, y) in [
            (vec![path!(selected_agenda)], self.selected_scroll),
            (
                vec![path!(day_agenda)],
                if self.agenda_upcoming {
                    self.agenda_scroll
                } else {
                    self.selected_scroll
                },
            ),
            (
                vec![
                    path!(wide.upcoming_agenda),
                    path!(landscape.upcoming_agenda),
                    path!(agenda_page.upcoming_agenda),
                ],
                self.agenda_scroll,
            ),
            (
                vec![
                    path!(search_panel.search_results),
                    path!(search_page.search_results),
                ],
                self.search_scroll,
            ),
        ] {
            for path in paths {
                if let Some(mut list) = self
                    .view
                    .widget(cx, path)
                    .borrow_mut::<CalendarAgendaList>()
                {
                    list.set_scroll(cx, y);
                }
            }
        }
        self.configure_stack_motion(cx);
        self.sync_modes(cx);
        for widget in descendants(&self.view) {
            if let Some(mut deck) = widget.borrow_mut::<CalendarBodyDeck>() {
                deck.cut(cx);
            }
        }
        for root in [path!(detail_content), path!(event_detail)] {
            let detail = self.view.widget(cx, root);
            let font = if compact { 12.75 } else { 11.25 };
            let height = if compact { 52.0 } else { 44.0 };
            for path in [
                path!(occurrence_dates),
                path!(calendar_row),
                path!(repeat_row),
            ] {
                let mut row = detail.widget(cx, path);
                script_apply_eval!(cx,row,{draw_text +: {text_style +: {font_size:#(font)}}});
                if path != path!(occurrence_dates) {
                    script_apply_eval!(cx,row,{height:#(height)});
                }
            }
        }
        self.view.redraw(cx);
    }
    fn sync_modes(&mut self, cx: &mut Cx) {
        self.view
            .drop_down(cx, path!(view_menu.menu))
            .set_selected_item(
                cx,
                match self.phone_mode {
                    PhoneMode::Month => 0,
                    PhoneMode::MonthList => 1,
                    PhoneMode::Day => 2,
                    PhoneMode::Agenda => 3,
                },
            );
        let id = match self.mode {
            CalendarMode::Month => live_id!(month),
            CalendarMode::Week => live_id!(week),
            CalendarMode::Day => live_id!(day),
        };
        for path in [path!(wide.body), path!(landscape.short_body)] {
            if let Some(mut deck) = self.view.widget(cx, path).borrow_mut::<CalendarBodyDeck>() {
                deck.activate(cx, id);
            }
        }
        for path in [
            path!(wide.toolbar.mode),
            path!(landscape.short_toolbar.mode),
        ] {
            if let Some(mut mode) = self.view.widget(cx, path).borrow_mut::<GlassSegmented>() {
                mode.set_selected(cx, self.mode.index());
            }
        }
        if matches!(self.phone_mode, PhoneMode::Month | PhoneMode::MonthList) {
            self.root_phone_mode = self.phone_mode;
        }
        let month_only = self.root_phone_mode == PhoneMode::Month;
        self.view
            .widget(cx, path!(selected_date))
            .set_visible(cx, !month_only);
        self.view
            .widget(cx, path!(selected_agenda))
            .set_visible(cx, !month_only);
        let mut dates = self.view.widget(cx, path!(month_dates));
        if month_only {
            script_apply_eval!(cx,dates,{height:mod.prelude.widgets_internal.Fill});
        } else {
            script_apply_eval!(cx,dates,{height:264.0});
        }
    }
    fn set_mode(&mut self, cx: &mut Cx, mode: CalendarMode) {
        self.mode = mode;
        if !self.layout.short_height {
            self.portrait_mode = mode;
        }
        self.sync_modes(cx);
        if let Some(doc) = &mut self.document {
            if doc.preferences.wide_mode != mode {
                doc.preferences.wide_mode = mode;
                doc.revision = doc.revision.saturating_add(1);
                self.queue_save(cx);
            }
        }
        self.view.redraw(cx);
    }
    fn select_day(&mut self, cx: &mut Cx, day: Day) {
        self.selected = day;
        self.displayed = civil::month_start(day);
        self.month_anchor_dom = civil::to_ymd(day).2;
        self.stack(cx).set_title(cx, live_id!(day_page), &format_heading_compact_day(day));
        self.view.redraw(cx);
    }
    fn shift_period(&mut self, cx: &mut Cx, dir: i32) {
        let path = if self.layout.kind == LayoutKind::Compact {
            path!(month_dates)
        } else if self.layout.short_height {
            path!(landscape.short_body)
        } else {
            path!(wide.body)
        };
        if let Some(mut deck) = self.view.widget(cx, path).borrow_mut::<CalendarBodyDeck>() {
            deck.transition(cx, dir);
        }
        let mode = if self.layout.kind == LayoutKind::Compact
            && matches!(self.phone_mode, PhoneMode::Month | PhoneMode::MonthList)
        {
            CalendarMode::Month
        } else {
            self.mode
        };
        match mode {
            CalendarMode::Month => {
                let (d, s, a) =
                    shift_month(self.displayed, self.selected, self.month_anchor_dom, dir);
                self.displayed = d;
                self.selected = s;
                self.month_anchor_dom = a;
            }
            CalendarMode::Week => self.select_day(cx, self.selected + 7 * dir),
            CalendarMode::Day => self.select_day(cx, self.selected + dir),
        }
        self.view.redraw(cx);
    }
    fn today(&mut self, cx: &mut Cx) {
        self.select_day(cx, self.clock.today);
        for w in descendants(&self.view) {
            if let Some(t) = w.borrow::<CalendarTimeline>() {
                t.scroll_to_today(cx);
            }
        }
    }
    fn open_day(&mut self, cx: &mut Cx, day: Day) {
        self.select_day(cx, day);
        if self.layout.kind == LayoutKind::Compact {
            self.phone_mode = PhoneMode::Day;
            self.push_page(cx, live_id!(day_page), &format_heading_compact_day(day));
        } else {
            self.set_mode(cx, CalendarMode::Day);
        }
    }
    fn open_agenda(&mut self, cx: &mut Cx, day: Day, upcoming: bool) {
        self.agenda_upcoming = upcoming;
        self.select_day(cx, day);
        if self.layout.kind == LayoutKind::Compact && upcoming {
            self.phone_mode = PhoneMode::Agenda;
            self.push_page(cx, live_id!(agenda_page), "Agenda");
        } else if self.layout.kind == LayoutKind::Compact {
            self.phone_mode = PhoneMode::MonthList;
            self.settle_stack(cx);
            self.stack_transition = self.pages.last().copied().map(|id| (id, false));
            self.pages.clear();
            self.stack(cx).pop_to_root(cx);
            self.sync_modes(cx);
        } else {
            self.show_overlay(cx, OverlayKind::Agenda);
        }
    }
    fn open_detail(&mut self, cx: &mut Cx, key: OccurrenceKey) {
        self.selected_occ = Some(key);
        if self.layout.kind == LayoutKind::Compact {
            self.push_page(cx, live_id!(detail_page), "Event");
        } else {
            self.show_overlay(cx, OverlayKind::Detail);
        }
        self.sync_detail(cx);
    }
    fn open_search(&mut self, cx: &mut Cx) {
        if self.layout.kind == LayoutKind::Compact {
            self.push_page(cx, live_id!(search_page), "Search");
        } else {
            self.show_overlay(cx, OverlayKind::Search);
        }
        self.sync_search_fields(cx);
    }
    fn open_calendars(&mut self, cx: &mut Cx) {
        if self.layout.kind == LayoutKind::Compact {
            self.push_page(cx, live_id!(calendars_page), "Calendars");
        } else {
            self.show_overlay(cx, OverlayKind::Calendars);
        }
    }
    fn queue_save(&mut self, cx: &mut Cx) {
        let Some(doc) = &self.document else {
            return;
        };
        if self.persist.on_mutate(doc.revision) {
            self.kick_save(cx);
        }
        self.view.redraw(cx);
    }
    fn kick_save(&mut self, cx: &mut Cx) {
        if self.persist.save_id.is_some() {
            return;
        }
        let Some(storage) = self.storage.clone() else {
            self.persist.status = PersistStatus::SaveError;
            return;
        };
        let Some(doc) = &self.document else {
            return;
        };
        match encode(doc) {
            Ok(bytes) => {
                let id = storage.set(cx, JAIL_KEY, bytes);
                self.persist.take_pending(id.0);
            }
            Err(e) => {
                log!("calendar: save failed: {e}");
                self.persist.status = PersistStatus::SaveError;
            }
        }
    }
    fn retry(&mut self, cx: &mut Cx) {
        if self.persist.status == PersistStatus::SaveError {
            self.kick_save(cx);
        } else if let Some(s) = &self.storage {
            self.persist.begin_load(s.get(cx, JAIL_KEY).0);
        }
        self.view.redraw(cx);
    }
    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.persist.on_load_response(response.request_id.0) {
                match &response.result {
                    Ok(StorageResult::Value(bytes)) => match load_bytes(bytes.as_deref()) {
                        LoadOutcome::Missing => {
                            self.document = Some(crate::seed::seed(self.clock.today));
                            self.persist.status = PersistStatus::Ready;
                            self.queue_save(cx);
                        }
                        LoadOutcome::Loaded(doc) => {
                            self.mode = doc.preferences.wide_mode;
                            self.portrait_mode = self.mode;
                            if self.layout.short_height && self.mode == CalendarMode::Month {
                                self.mode = CalendarMode::Week;
                            }
                            self.document = Some(doc);
                            self.persist.status = PersistStatus::Ready;
                            self.sync_modes(cx);
                        }
                        LoadOutcome::Error { message } => {
                            log!("calendar: load error: {message}");
                            self.persist.preserved_bytes = bytes.clone();
                            self.persist.status = PersistStatus::LoadError;
                        }
                    },
                    Err(e) => {
                        log!("calendar: load failed: {e}");
                        self.persist.status = PersistStatus::LoadError;
                    }
                    _ => self.persist.status = PersistStatus::LoadError,
                }
            }
            match self
                .persist
                .on_save_response(response.request_id.0, response.result.is_ok())
            {
                SaveAck::WriteNext => self.kick_save(cx),
                SaveAck::Idle => {
                    self.saved_timer = Some(cx.start_timeout(2.0));
                }
                SaveAck::Failed => log!("calendar: changes not saved"),
                SaveAck::Ignored => {}
            }
        }
        self.view.redraw(cx);
    }
    fn open_editor(&mut self, cx: &mut Cx, existing: Option<EventId>, minute: Option<u16>) {
        let Some(doc) = &self.document else {
            return;
        };
        let original = existing.and_then(|id| event_by_id(doc, id).cloned());
        let value = original.clone().unwrap_or_else(|| {
            default_new_event(
                self.clock,
                self.selected,
                doc.preferences.default_calendar,
                minute,
            )
        });
        let series = value.repeat != Repeat::None;
        let fields = DraftFields::from_event(&value);
        self.original_fields = Some(fields.clone());
        self.fields = Some(fields);
        self.draft = Some(EventDraft {
            editing: existing,
            value,
            original,
            end_was_edited: false,
        });
        self.errors = DraftErrors::default();
        self.show_overlay(cx, OverlayKind::Editor);
        self.hydrate_editor(cx);
        let panel = self.panel(cx);
        panel.label(cx, path!(editor_header.editor_title)).set_text(
            cx,
            if existing.is_some() {
                if series {
                    "Edit Series"
                } else {
                    "Edit Event"
                }
            } else {
                "New Event"
            },
        );
        panel
            .button(cx, path!(save))
            .set_text(cx, if existing.is_some() { "Save" } else { "Add" });
        panel
            .widget(cx, path!(series_notice))
            .set_visible(cx, series);
        if series {
            let anchor = self.draft.as_ref().unwrap().value.timing.start_day();
            panel.label(cx, path!(series_notice)).set_text(
                cx,
                &format!(
                    "Series starts {}. Changes apply to every occurrence.",
                    civil::format_iso(anchor)
                ),
            );
        }
        self.validate_fields(cx, false);
    }
    fn hydrate_editor(&self, cx: &mut Cx) {
        let Some(f) = &self.fields else {
            return;
        };
        let panel = self.panel(cx);
        panel
            .text_input(cx, path!(title_input))
            .set_text(cx, &f.title);
        panel
            .drop_down(cx, path!(calendar_choice))
            .set_selected_item(cx, f.calendar_index);
        panel
            .check_box(cx, path!(all_day))
            .set_active(cx, f.all_day, Animate::No);
        panel
            .text_input(cx, path!(starts_row.date_input))
            .set_text(cx, &f.start_date);
        panel
            .text_input(cx, path!(starts_row.time_input))
            .set_text(cx, &f.start_time);
        panel
            .text_input(cx, path!(ends_row.date_input))
            .set_text(cx, &f.end_date);
        panel
            .text_input(cx, path!(ends_row.time_input))
            .set_text(cx, &f.end_time);
        panel
            .drop_down(cx, path!(repeat_choice))
            .set_selected_item(cx, f.repeat_index);
        panel
            .text_input(cx, path!(notes_input))
            .set_text(cx, &f.notes);
    }
    fn pull_fields(&mut self, cx: &mut Cx) {
        let panel = self.panel(cx);
        let Some(f) = self.fields.as_mut() else {
            return;
        };
        f.title = panel.text_input(cx, path!(title_input)).text();
        f.notes = panel.text_input(cx, path!(notes_input)).text();
        f.calendar_index = panel.drop_down(cx, path!(calendar_choice)).selected_item();
        f.repeat_index = panel.drop_down(cx, path!(repeat_choice)).selected_item();
        let all_day = panel.check_box(cx, path!(all_day)).active(cx);
        f.start_date = panel.text_input(cx, path!(starts_row.date_input)).text();
        f.start_time = panel.text_input(cx, path!(starts_row.time_input)).text();
        f.end_date = panel.text_input(cx, path!(ends_row.date_input)).text();
        f.end_time = panel.text_input(cx, path!(ends_row.time_input)).text();
        if f.all_day != all_day {
            f.set_all_day(all_day);
            panel.text_input(cx, path!(ends_row.date_input)).set_text(cx, &f.end_date);
        }
        self.validate_fields(cx, true);
    }
    fn validate_fields(&mut self, cx: &mut Cx, show_errors: bool) {
        if let (Some(draft), Some(fields), Some(doc)) =
            (&mut self.draft, &mut self.fields, &self.document)
        {
            self.errors = apply_draft_fields(draft, fields, &doc.calendars);
            // Preserve duration while changing Starts, but do not rewrite focused input or invalid strings.
            if !draft.end_was_edited
                && self.errors.start.is_none()
                && self.errors.end.is_none()
                && !fields.all_day
            {
                if let Timing::Timed { end, .. } = draft.value.timing {
                    fields.end_date = civil::format_iso(end.day);
                    fields.end_time = end.format_hm();
                }
            }
        }
        let panel = self.panel(cx);
        panel
            .widget(cx, path!(save))
            .set_disabled(cx, !self.errors.is_empty());
        for (path, error) in [
            (path!(title_error), self.errors.title.as_deref()),
            (path!(starts_row.field_error), self.errors.start.as_deref()),
            (path!(ends_row.field_error), self.errors.end.as_deref()),
            (path!(validation), self.errors.calendar.as_deref()),
        ] {
            panel
                .widget(cx, path)
                .set_visible(cx, show_errors && error.is_some());
            panel.label(cx, path).set_text(cx, error.unwrap_or(""));
        }
        if let Some(f) = &self.fields {
            for path in [path!(starts_row.time_input), path!(ends_row.time_input)] {
                panel.widget(cx, path).set_visible(cx, !f.all_day);
            }
            if self.draft.as_ref().is_some_and(|d| !d.end_was_edited) && self.errors.end.is_none() {
                panel
                    .text_input(cx, path!(ends_row.date_input))
                    .set_text(cx, &f.end_date);
                panel
                    .text_input(cx, path!(ends_row.time_input))
                    .set_text(cx, &f.end_time);
            }
        }
        self.view.redraw(cx);
    }
    fn fields_dirty(&self) -> bool {
        match (&self.fields, &self.original_fields) {
            (Some(a), Some(b)) => {
                a.title != b.title
                    || a.calendar_index != b.calendar_index
                    || a.all_day != b.all_day
                    || a.start_date != b.start_date
                    || a.start_time != b.start_time
                    || a.end_date != b.end_date
                    || a.end_time != b.end_time
                    || a.repeat_index != b.repeat_index
                    || a.notes != b.notes
            }
            _ => false,
        }
    }
    fn request_close_editor(&mut self, cx: &mut Cx) {
        if self.fields_dirty() {
            self.confirm = Confirm::Discard;
            self.confirm_return = OverlayKind::Editor;
            self.show_confirmation(cx, "Discard changes?", "Keep Editing", "Discard");
        } else {
            self.finish_editor(cx);
        }
    }
    fn finish_editor(&mut self, cx: &mut Cx) {
        self.draft = None;
        self.fields = None;
        self.original_fields = None;
        self.confirm = Confirm::None;
        self.close_overlay(cx);
    }
    fn save_editor(&mut self, cx: &mut Cx) {
        self.pull_fields(cx);
        if !self.errors.is_empty() {
            return;
        }
        let (Some(doc), Some(draft)) = (&mut self.document, &self.draft) else {
            return;
        };
        match commit_draft(doc, draft) {
            Ok(id) => {
                let key = OccurrenceKey {
                    event_id: id,
                    start_day: draft.value.timing.start_day(),
                };
                self.selected_occ = Some(key);
                self.finish_editor(cx);
                self.queue_save(cx);
                self.sync_detail(cx);
            }
            Err(message) => {
                let panel = self.panel(cx);
                panel.widget(cx, path!(validation)).set_visible(cx, true);
                panel.label(cx, path!(validation)).set_text(cx, &message);
            }
        }
    }
    fn show_confirmation(&mut self, cx: &mut Cx, message: &str, cancel: &str, confirm: &str) {
        let panel = self.view.widget(cx, path!(confirmation));
        panel.label(cx, path!(message)).set_text(cx, message);
        panel.button(cx, path!(cancel)).set_text(cx, cancel);
        panel.button(cx, path!(confirm)).set_text(cx, confirm);
        self.show_overlay(cx, OverlayKind::Confirmation);
    }
    fn request_delete(&mut self, cx: &mut Cx) {
        let Some(key) = self.selected_occ else {
            return;
        };
        let Some(event) = self
            .document
            .as_ref()
            .and_then(|d| event_by_id(d, key.event_id))
        else {
            return;
        };
        let series = event.repeat != Repeat::None;
        self.confirm = Confirm::Delete(key.event_id);
        self.confirm_return = if self.layout.kind == LayoutKind::Compact {
            OverlayKind::None
        } else {
            OverlayKind::Detail
        };
        self.show_confirmation(
            cx,
            if series {
                "Delete entire series?"
            } else {
                "Delete this event?"
            },
            "Cancel",
            if series { "Delete Series" } else { "Delete" },
        );
    }
    fn confirm_no(&mut self, cx: &mut Cx) {
        self.confirm = Confirm::None;
        if self.confirm_return == OverlayKind::None {
            self.close_overlay(cx);
        } else {
            self.show_overlay(cx, self.confirm_return);
        }
    }
    fn confirm_yes(&mut self, cx: &mut Cx) {
        match self.confirm {
            Confirm::Discard => self.finish_editor(cx),
            Confirm::Delete(id) => {
                if let Some(doc) = &mut self.document {
                    delete_event(doc, id);
                }
                self.confirm = Confirm::None;
                self.selected_occ = None;
                self.close_overlay(cx);
                if self.layout.kind == LayoutKind::Compact {
                    self.back(cx);
                }
                self.queue_save(cx);
            }
            Confirm::None => {}
        }
    }
    fn sync_detail(&self, cx: &mut Cx) {
        let Some(key) = self.selected_occ else {
            return;
        };
        let Some(doc) = &self.document else {
            return;
        };
        let Some(event) = event_by_id(doc, key.event_id) else {
            return;
        };
        let Some(occ) = occurrence_for_key(doc, key) else {
            return;
        };
        let cal = calendar_by_id(doc, event.calendar_id);
        let roots = [
            self.view.widget(cx, path!(detail_content)),
            self.view.widget(cx, path!(event_detail)),
        ];
        for panel in roots {
            panel
                .label(cx, path!(event_title))
                .set_text(cx, &event.title);
            panel
                .label(cx, path!(occurrence_dates))
                .set_text(cx, &occurrence_dates(occ.timing));
            panel
                .label(cx, path!(calendar_row))
                .set_text(cx, cal.map(|c| c.name.as_str()).unwrap_or("Calendar"));
            panel.label(cx, path!(repeat_row)).set_text(
                cx,
                if event.repeat == Repeat::None {
                    "Does not repeat"
                } else {
                    event.repeat.description()
                },
            );
            panel
                .label(cx, path!(event_notes))
                .set_text(cx, &event.notes);
            panel
                .widget(cx, path!(event_notes))
                .set_visible(cx, !event.notes.is_empty());
            let height = if self.layout.kind == LayoutKind::Compact {
                52.0
            } else {
                44.0
            };
            for path in [path!(calendar_row), path!(repeat_row)] {
                let mut row = panel.widget(cx, path);
                script_apply_eval!(cx,row,{height:#(height)});
            }
        }
        self.view.button(cx, path!(detail_panel.edit)).set_text(
            cx,
            if event.repeat == Repeat::None {
                "Edit"
            } else {
                "Edit Series"
            },
        );
    }
    fn sync_search_fields(&mut self, cx: &mut Cx) {
        let active = if self.layout.kind == LayoutKind::Compact {
            path!(search_page.search_query)
        } else {
            path!(search_panel.search_query)
        };
        if self.query_editor.is_empty() {
            self.query_editor = self.view.widget(cx, active).widget(cx, path!(query));
        }
        // Move the same editor, including selection, IME and undo state, between hosts.
        for path in [path!(search_page.search_query), path!(search_panel.search_query)] {
            if let Some(mut host) = self.view.widget(cx, path).borrow_mut::<View>() {
                host.children.retain(|(id, _)| *id != live_id!(query));
                if path == active {
                    add_child(&mut host, cx, live_id!(query), self.query_editor.clone());
                } else {
                    cx.widget_tree_mark_dirty(host.widget_uid());
                }
            }
        }
        let editor = self.query_editor.as_text_input();
        if editor.text() != self.search_query {
            editor.set_text(cx, &self.search_query);
        }
    }
    fn populate(&mut self, cx: &mut Cx) {
        let colors = cx.with_vm(CalendarColors::resolve);
        if !self.theme_initialized {
            self.recipe_theme = colors;
        }
        if !self.theme_initialized || self.theme != colors {
            self.styled.clear();
            self.theme = colors;
            self.theme_initialized = true;
        }
        let mut themed: Vec<_> = self.view.children.iter().map(|(_, w)| (w.clone(), colors.paper)).collect();
        let mut next_widget = 0;
        while next_widget < themed.len() {
            let (widget, surface) = themed[next_widget].clone();
            next_widget += 1;
            let surface = retint_widget(cx, &widget, self.recipe_theme, colors, surface,
                self.styled.insert(widget.widget_uid()));
            widget.try_children(&mut |_, child| themed.push((child, surface)));
            if let Some(mut w) = widget.borrow_mut::<CalendarBodyDeck>() {
                w.reduced_motion = self.reduced_motion;
            }
            if let Some(mut w) = widget.borrow_mut::<CalendarOverlayHost>() {
                w.reduced_motion = self.reduced_motion;
            }
            if let Some(mut w) = widget.borrow_mut::<CalendarControlGroup>() {
                w.reduced_motion = self.reduced_motion;
            }
            if let Some(mut w) = widget.borrow_mut::<CalendarHitRow>() {
                w.reduced_motion = self.reduced_motion;
            }
            if let Some(mut w) = widget.borrow_mut::<CalendarDateMark>() {
                w.reduced_motion = self.reduced_motion;
            };
        }
        let color = colors.paper;
        script_apply_eval!(cx,self.view,{draw_bg.color:#(color)});
        let (year, month, _) = civil::to_ymd(self.displayed);
        self.view.label(cx, path!(wide.period_title)).set_text(
            cx,
            &if self.mode == CalendarMode::Month {
                civil::month_name(month).to_string()
            } else {
                period_heading(self.mode, self.displayed, self.selected)
            },
        );
        self.view.label(cx, path!(wide.period_year)).set_text(
            cx,
            &if self.mode == CalendarMode::Month {
                year.to_string()
            } else {
                String::new()
            },
        );
        self.view.label(cx, path!(landscape.period)).set_text(
            cx,
            &period_heading(self.mode, self.displayed, self.selected),
        );
        self.view
            .label(cx, path!(month_title))
            .set_text(cx, civil::month_name(month));
        self.view
            .button(cx, path!(navigation.year))
            .set_text(cx, &year.to_string());
        self.view
            .label(cx, path!(selected_date))
            .set_text(cx, &format_heading_compact_day(self.selected));
        self.view.label(cx, path!(agenda_date)).set_text(
            cx,
            &if self.agenda_upcoming {
                format!(
                    "Upcoming · {} – {}",
                    civil::format_iso(self.selected),
                    civil::format_iso(self.selected + 30)
                )
            } else {
                format_heading_compact_day(self.selected)
            },
        );
        let grid = month_grid_monday(year, month);
        let first = grid[0][0].day;
        let doc = self.document.as_ref();
        let month_events = doc
            .map(|d| {
                present(
                    d,
                    occurrences_in_range(d, first, first + 42, true, UI_EXPAND_CAP).items,
                )
            })
            .unwrap_or_default();
        let day_events = doc
            .map(|d| {
                present(
                    d,
                    occurrences_for_day(d, self.selected, true, UI_EXPAND_CAP).items,
                )
            })
            .unwrap_or_default();
        let monday = monday_of(self.selected);
        let week_events = doc
            .map(|d| {
                present(
                    d,
                    occurrences_in_range(d, monday, monday + 7, true, UI_EXPAND_CAP).items,
                )
            })
            .unwrap_or_default();
        let upcoming = doc
            .map(|d| {
                present(
                    d,
                    occurrences_in_range(d, self.selected, self.selected + 31, true, UI_EXPAND_CAP)
                        .items,
                )
            })
            .unwrap_or_default();
        let found_list = doc.map(|d| search(d, &self.search_query, self.clock.today, true))
            .unwrap_or_else(OccurrenceList::empty);
        let search_truncated = found_list.truncated;
        let found = doc.map(|d| present(d, found_list.items)).unwrap_or_default();
        let none = doc.is_some_and(|d| visible_calendar_count(d) == 0);
        let empty = if self.persist.status == PersistStatus::Loading {
            "Loading calendar…"
        } else if self.persist.status == PersistStatus::LoadError {
            "Could not load calendar. Use Retry."
        } else if none {
            "No calendars selected"
        } else {
            "No events"
        };
        let month_frame = MonthFrame {
            displayed: self.displayed,
            selected: self.selected,
            today: self.clock.today,
            events: month_events,
        };
        let day_frame = TimelineFrame {
            days: vec![self.selected],
            events: day_events.clone(),
            clock: self.clock,
            selected: self.selected,
            gutter: if self.layout.kind == LayoutKind::Compact {
                52.0
            } else {
                56.0
            },
            compact: self.layout.kind == LayoutKind::Compact,
            short: self.layout.short_height,
        };
        let week_frame = TimelineFrame {
            days: (0..7).map(|i| monday + i).collect(),
            events: week_events,
            ..day_frame.clone()
        };
        // Explicitly bind each agenda by its semantic path, never by an arbitrary list fallback.
        for (paths, events, grouped, message, add) in [
            (
                vec![path!(selected_agenda)],
                day_events.clone(),
                false,
                empty,
                !none,
            ),
            (
                vec![path!(day_agenda)],
                if self.agenda_upcoming {
                    upcoming.clone()
                } else {
                    day_events
                },
                self.agenda_upcoming,
                empty,
                !none,
            ),
            (
                vec![
                    path!(wide.upcoming_agenda),
                    path!(landscape.upcoming_agenda),
                    path!(agenda_page.upcoming_agenda),
                ],
                upcoming,
                true,
                empty,
                !none,
            ),
            (
                vec![
                    path!(search_panel.search_results),
                    path!(search_page.search_results),
                ],
                found,
                true,
                if none {
                    "No calendars selected"
                } else if self.search_query.trim().is_empty() {
                    "Search event titles, notes, or calendars"
                } else {
                    "No matching events"
                },
                false,
            ),
        ] {
            for path in paths {
                if let Some(mut list) = self
                    .view
                    .widget(cx, path)
                    .borrow_mut::<CalendarAgendaList>()
                {
                    list.frame = AgendaFrame {
                        events: events.clone(),
                        grouped,
                        empty: message.into(),
                        allow_add: add && doc.is_some(),
                    };
                }
            }
        }
        for path in [path!(wide.week), path!(landscape.week)] {
            if let Some(mut t) = self.view.widget(cx, path).borrow_mut::<CalendarTimeline>() {
                t.frame = week_frame.clone();
            }
        }
        for path in [path!(wide.day), path!(landscape.day), path!(day_timeline)] {
            if let Some(mut t) = self.view.widget(cx, path).borrow_mut::<CalendarTimeline>() {
                t.frame = day_frame.clone();
            }
        }
        if let Some(mut strip) = self
            .view
            .widget(cx, path!(date_strip))
            .borrow_mut::<CalendarTimelineHeaders>()
        {
            strip.frame = day_frame;
        }
        for w in descendants(&self.view) {
            if let Some(mut m) = w.borrow_mut::<CalendarMonthCanvas>() {
                m.frame = month_frame.clone();
            }
            if let Some(mut m) = w.borrow_mut::<CalendarMiniMonth>() {
                m.frame = MonthFrame {
                    events: Vec::new(),
                    ..month_frame.clone()
                };
            }
        }
        let range = format!(
            "{} – {}",
            civil::format_iso(self.selected),
            civil::format_iso(self.selected + 30)
        );
        for path in [
            path!(wide.agenda_range),
            path!(landscape.agenda_range),
            path!(agenda_page.agenda_range),
        ] {
            self.view.label(cx, path).set_text(cx, &range);
        }
        let range = format!(
            "{} – {} · {}",
            civil::format_iso(self.clock.today - SEARCH_BACK_DAYS),
            civil::format_iso(self.clock.today + SEARCH_FORWARD_DAYS - 1),
            if search_truncated { "First 100 results · more matches" } else { "up to 100 results" }
        );
        for path in [
            path!(search_panel.search_range),
            path!(search_page.search_range),
        ] {
            self.view.label(cx, path).set_text(cx, &range);
        }
        self.sync_calendar_rows(cx);
        self.sync_status(cx);
        self.sync_detail(cx);
        self.friendly_dates(cx);
    }
    fn friendly_dates(&self, cx: &mut Cx) {
        if self.draft.is_none() {
            return;
        }
        for path in [path!(starts_row), path!(ends_row)] {
            let row = self.panel(cx).widget(cx, path);
            let input = row.text_input(cx, path!(date_input));
            let day = civil::parse_iso(&input.text());
            let summary = day.map(|d| {
                let (_, m, n) = civil::to_ymd(d);
                format!(
                    "{} {n}, {}",
                    civil::MONTH_ABBR[(m - 1) as usize],
                    civil::to_ymd(d).0
                )
            });
            let show = !cx.has_key_focus(input.area()) && summary.is_some();
            row.widget(cx, path!(date_summary)).set_visible(cx, show);
            row.label(cx, path!(date_summary))
                .set_text(cx, summary.as_deref().unwrap_or(""));
            let mut input = row.widget(cx, path!(date_input));
            let color = if show {
                Vec4f::default()
            } else {
                self.theme.ink
            };
            script_apply_eval!(cx,input,{draw_text.color:#(color)});
        }
    }
    fn sync_calendar_rows(&self, cx: &mut Cx) {
        let Some(doc) = &self.document else {
            return;
        };
        for root in [
            path!(sidebar.calendar_rows),
            path!(calendars_page.calendar_choices),
            path!(calendars_panel.calendar_choices),
        ] {
            let colors = if root == path!(sidebar.calendar_rows) {
                self.theme.on_surface(self.theme.sidebar)
            } else { self.theme };
            let parent = self.view.widget(cx, root);
            for (i, path) in [path!(cal0), path!(cal1), path!(cal2), path!(cal3)]
                .iter()
                .enumerate()
            {
                let Some(cal) = doc.calendars.get(i) else {
                    continue;
                };
                let mut row = parent.widget(cx, path);
                let height = if root == path!(sidebar.calendar_rows) {
                    36.0
                } else {
                    44.0
                };
                script_apply_eval!(cx,row,{height:#(height)});
                target(&row, CalendarAction::Calendar(i));
                label(&row, cx, path!(calendar_name), &cal.name, self.theme.ink);
                let color = colors.category(cal.colour);
                if let Some(mut mark) = row
                    .widget(cx, path!(indicator))
                    .borrow_mut::<CalendarDateMark>()
                {
                    mark.set_checked(cx, cal.visible, color);
                }
            }
        }
    }
    fn sync_status(&self, cx: &mut Cx) {
        let retry = matches!(
            self.persist.status,
            PersistStatus::LoadError | PersistStatus::SaveError
        );
        let message = match self.persist.status {
            PersistStatus::LoadError => "Could not load",
            PersistStatus::SaveError => "Changes not saved",
            other => other.label(),
        };
        for root in [
            path!(sidebar.storage_status),
            path!(storage_notice.storage_status),
        ] {
            let panel = self.view.widget(cx, root);
            panel.label(cx, path!(status)).set_text(cx, message);
            panel.widget(cx, path!(retry)).set_visible(cx, retry);
        }
        let notice = self.view.widget(cx, path!(storage_notice));
        notice.set_visible(
            cx,
            self.persist.status != PersistStatus::Ready
                && (self.layout.kind == LayoutKind::Compact || self.layout.short_height || retry),
        );
        let width = 320.0_f64.min(self.dimensions.x - 32.0).max(44.0);
        let mut notice = notice;
        let origin = self.view.area().rect(cx).pos;
        script_apply_eval!(cx,notice,{width:#(width) abs_pos:#(origin+dvec2((self.dimensions.x-width-16.0).max(0.0),8.0))});
    }
    fn clicked(&self, cx: &mut Cx, path: &[LiveId], actions: &Actions) -> bool {
        self.view.button(cx, path).clicked(actions)
    }
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let overlay = self.current_overlay(cx);
        if overlay == OverlayKind::Confirmation {
            if self.clicked(cx, path!(confirmation.cancel), actions) {
                self.confirm_no(cx);
            }
            if self.clicked(cx, path!(confirmation.confirm), actions) {
                self.confirm_yes(cx);
            }
            return;
        }
        if overlay == OverlayKind::Editor {
            let panel = self.panel(cx);
            if panel.button(cx, path!(cancel)).clicked(actions) {
                self.request_close_editor(cx);
                return;
            }
            if panel.button(cx, path!(save)).clicked(actions) {
                self.save_editor(cx);
                return;
            }
            let end_changed = panel
                .text_input(cx, path!(ends_row.date_input))
                .changed(actions)
                .is_some()
                || panel
                    .text_input(cx, path!(ends_row.time_input))
                    .changed(actions)
                    .is_some();
            let all_changed = panel
                .check_box(cx, path!(all_day))
                .changed(actions)
                .is_some();
            if end_changed || all_changed {
                if let Some(d) = &mut self.draft {
                    d.end_was_edited = true;
                }
            }
            let input_changed = [
                path!(title_input),
                path!(notes_input),
                path!(starts_row.date_input),
                path!(starts_row.time_input),
                path!(ends_row.date_input),
                path!(ends_row.time_input),
            ]
            .iter()
            .any(|p| panel.text_input(cx, p).changed(actions).is_some());
            if input_changed
                || all_changed
                || panel
                    .drop_down(cx, path!(calendar_choice))
                    .changed(actions)
                    .is_some()
                || panel
                    .drop_down(cx, path!(repeat_choice))
                    .changed(actions)
                    .is_some()
            {
                self.pull_fields(cx);
            }
        } else {
            for path in [
                path!(wide.toolbar),
                path!(navigation),
                path!(short_toolbar),
                path!(bottom_toolbar),
                path!(day_toolbar),
                path!(agenda_toolbar),
            ] {
                let parent = self.view.widget(cx, path);
                if parent.button(cx, path!(previous)).clicked(actions) {
                    self.shift_period(cx, -1);
                }
                if parent.button(cx, path!(next)).clicked(actions) {
                    self.shift_period(cx, 1);
                }
                if parent.button(cx, path!(today)).clicked(actions)
                    || parent.button(cx, path!(year)).clicked(actions)
                {
                    self.today(cx);
                }
                if parent.button(cx, path!(add)).clicked(actions) {
                    self.last_anchor = parent.button(cx, path!(add)).area().rect(cx);
                    self.open_editor(cx, None, None);
                }
                if parent.button(cx, path!(search)).clicked(actions) {
                    self.last_anchor = parent.button(cx, path!(search)).area().rect(cx);
                    self.open_search(cx);
                }
                if parent.button(cx, path!(calendars)).clicked(actions) {
                    self.last_anchor = parent.button(cx, path!(calendars)).area().rect(cx);
                    self.open_calendars(cx);
                }
                let selected = parent
                    .widget(cx, path!(mode))
                    .borrow::<GlassSegmented>()
                    .and_then(|m| {
                        if m.changed(actions) {
                            Some(m.selected())
                        } else {
                            None
                        }
                    });
                if let Some(i) = selected {
                    self.set_mode(cx, CalendarMode::from_index(i));
                }
            }
            if let Some(i) = self
                .view
                .drop_down(cx, path!(view_menu.menu))
                .changed(actions)
            {
                self.phone_mode = match i {
                    0 => PhoneMode::Month,
                    2 => PhoneMode::Day,
                    3 => PhoneMode::Agenda,
                    _ => PhoneMode::MonthList,
                };
                match self.phone_mode {
                    PhoneMode::Day => self.open_day(cx, self.selected),
                    PhoneMode::Agenda => self.open_agenda(cx, self.selected, true),
                    _ => self.sync_modes(cx),
                }
            }
            for root in [path!(search_page), path!(search_panel)] {
                if let Some(text) = self
                    .view
                    .widget(cx, root)
                    .text_input(cx, path!(query))
                    .changed(actions)
                {
                    self.search_query = text;
                }
            }
            for root in [
                path!(detail_panel),
                path!(search_panel),
                path!(calendars_panel),
                path!(agenda_panel),
            ] {
                let panel = self.view.widget(cx, root);
                if panel.button(cx, path!(close)).clicked(actions) {
                    self.close_overlay(cx);
                }
                if panel.button(cx, path!(edit)).clicked(actions) {
                    if let Some(k) = self.selected_occ {
                        self.open_editor(cx, Some(k.event_id), None);
                    }
                }
            }
            for root in [path!(detail_content), path!(event_detail)] {
                if self
                    .view
                    .widget(cx, root)
                    .button(cx, path!(delete_event))
                    .clicked(actions)
                {
                    self.request_delete(cx);
                }
            }
            for id in [
                live_id!(day_page),
                live_id!(agenda_page),
                live_id!(detail_page),
                live_id!(calendars_page),
                live_id!(search_page),
            ] {
                let page = self.stack(cx).view_by_id(cx, id);
                if page.button(cx, path!(header_action)).clicked(actions) {
                    if id == live_id!(detail_page) {
                        if let Some(k) = self.selected_occ {
                            self.open_editor(cx, Some(k.event_id), None);
                        }
                    } else {
                        self.back(cx);
                    }
                }
            }
        }
        for path in [
            path!(sidebar.storage_status.retry),
            path!(storage_notice.retry),
        ] {
            if self.clicked(cx, path, actions) {
                self.retry(cx);
            }
        }
        for action in actions {
            let Some(wa) = action.as_widget_action() else {
                continue;
            };
            if matches!(
                wa.cast::<StackNavigationAction>(),
                StackNavigationAction::Pop
            ) {
                self.back(cx);
                continue;
            }
            if matches!(wa.cast::<OverlayAction>(), OverlayAction::Dismiss) {
                self.dismiss(cx);
                continue;
            }
            let target = wa.cast::<CalendarAction>();
            if target != CalendarAction::None && self.draft.is_none() {
                let widget = cx.widget_tree().widget(wa.widget_uid);
                self.last_anchor = widget.area().rect(cx);
                match target {
                    CalendarAction::SelectDay(d) => self.select_day(cx, d),
                    CalendarAction::OpenDay(d) => self.open_day(cx, d),
                    CalendarAction::Event(k) => self.open_detail(cx, k),
                    CalendarAction::Agenda(d) => self.open_agenda(cx, d, false),
                    CalendarAction::Calendar(i) => {
                        if let Some(doc) = &mut self.document {
                            if let Some(cal) = doc.calendars.get(i).cloned() {
                                set_calendar_visible(doc, cal.id, !cal.visible);
                                self.queue_save(cx);
                            }
                        }
                    }
                    CalendarAction::Period(dir) => self.shift_period(cx, dir),
                    CalendarAction::Create { day, minute } => {
                        self.select_day(cx, day);
                        self.open_editor(cx, None, Some(minute));
                    }
                    CalendarAction::Add => self.open_editor(cx, None, None),
                    CalendarAction::None => {}
                }
            }
        }
        self.view.redraw(cx);
    }
    fn dismiss(&mut self, cx: &mut Cx) {
        match self.current_overlay(cx) {
            OverlayKind::Editor => self.request_close_editor(cx),
            OverlayKind::Confirmation => self.confirm_no(cx),
            OverlayKind::None => {
                if !self.pages.is_empty() {
                    self.back(cx);
                }
            }
            _ => self.close_overlay(cx),
        }
    }
    fn keys(&mut self, cx: &mut Cx, event: &Event) {
        let Event::KeyDown(e) = event else {
            return;
        };
        if e.key_code == KeyCode::Escape {
            self.dismiss(cx);
            return;
        }
        if self.draft.is_some()
            || self.current_overlay(cx) != OverlayKind::None
            || !self.pages.is_empty()
        {
            return;
        }
        match e.key_code {
            KeyCode::ArrowLeft => self.select_day(cx, self.selected - 1),
            KeyCode::ArrowRight => self.select_day(cx, self.selected + 1),
            KeyCode::ArrowUp => self.select_day(cx, self.selected - 7),
            KeyCode::ArrowDown => self.select_day(cx, self.selected + 7),
            KeyCode::PageUp => self.shift_period(cx, -1),
            KeyCode::PageDown => self.shift_period(cx, 1),
            KeyCode::ReturnKey => self.open_day(cx, self.selected),
            KeyCode::KeyN if e.modifiers.logo || e.modifiers.control => {
                self.open_editor(cx, None, None)
            }
            _ => {}
        }
    }
}
impl Widget for CalendarView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if self
            .tick
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
            || matches!(
                event,
                Event::Resume | Event::Foreground | Event::WindowGotFocus(_)
            )
        {
            self.clock = ClockSnapshot::from_epoch_secs(Cx::time_now() as i64);
            self.view.redraw(cx);
        }
        if self
            .saved_timer
            .as_ref()
            .is_some_and(|t| t.is_event(event).is_some())
        {
            self.saved_timer = None;
            if self.persist.status == PersistStatus::Saved {
                self.persist.status = PersistStatus::Ready;
            }
            self.view.redraw(cx);
        }
        if let Event::VirtualKeyboard(e) = event {
            match e {
                VirtualKeyboardEvent::WillShow {
                    time,
                    height,
                    duration,
                    ease,
                }
                | VirtualKeyboardEvent::WillHide {
                    time,
                    height,
                    duration,
                    ease,
                } => {
                    self.keyboard_motion = KeyboardMotion {
                        from: self.keyboard_height,
                        target: *height,
                        start: *time,
                        duration: *duration,
                        ease: ease.clone(),
                        next: cx.new_next_frame(),
                    };
                }
                VirtualKeyboardEvent::DidShow { height, .. } => self.keyboard_height = *height,
                VirtualKeyboardEvent::DidHide { .. } => self.keyboard_height = 0.0,
            }
            self.view.redraw(cx);
        }
        if let Some(frame) = self.keyboard_motion.next.is_event(event) {
            let m = &self.keyboard_motion;
            let t = ((frame.time - m.start) / m.duration.max(0.001)).clamp(0.0, 1.0);
            self.keyboard_height = m.from + (m.target - m.from) * m.ease.map(t);
            if t < 1.0 {
                self.keyboard_motion.next = cx.new_next_frame();
            }
            self.view.redraw(cx);
        }
        if matches!(event, Event::VirtualKeyboard(_))
            || self.keyboard_motion.next.is_event(event).is_some()
            || self.keyboard_height > 0.0 && matches!(event, Event::NextFrame(_))
        {
            if let Some(mut h) = self.overlay(cx).borrow_mut::<CalendarOverlayHost>() {
                h.keyboard_occlusion = self.keyboard_height;
            }
            if let Some(mut scroll) = self
                .panel(cx)
                .widget(cx, path!(editor_scroll))
                .borrow_mut::<CalendarEditorScroll>()
            {
                scroll.reveal_focus = true;
            }
        }
        if let Event::KeyDown(e) = event {
            if e.key_code == KeyCode::Tab && self.current_overlay(cx) != OverlayKind::None {
                let id = self
                    .overlay(cx)
                    .borrow::<CalendarOverlayHost>()
                    .map(|h| h.id())
                    .unwrap_or_default();
                let panel = self.view.widget(cx, &[id]);
                let mut nodes = Vec::new();
                panel.try_children(&mut |_, w| nodes.push(w));
                let mut i = 0;
                while i < nodes.len() {
                    let w = nodes[i].clone();
                    w.try_children(&mut |_, c| nodes.push(c));
                    i += 1;
                }
                let controls: Vec<_> = nodes
                    .into_iter()
                    .filter(|w| {
                        w.visible()
                            && !w.area().is_empty()
                            && (w.borrow::<Button>().is_some()
                                || w.borrow::<TextInput>().is_some()
                                || w.borrow::<DropDown>().is_some()
                                || w.borrow::<CheckBox>().is_some())
                    })
                    .collect();
                if !controls.is_empty() {
                    let at = controls.iter().position(|w| cx.has_key_focus(w.area()));
                    let index = if e.modifiers.shift {
                        at.map(|i| (i + controls.len() - 1) % controls.len())
                            .unwrap_or(controls.len() - 1)
                    } else {
                        at.map(|i| (i + 1) % controls.len()).unwrap_or(0)
                    };
                    if controls[index].borrow::<TextInput>().is_some() {
                        controls[index].as_text_input().take_key_focus(cx);
                    } else {
                        cx.set_key_focus(controls[index].area());
                    }
                }
                return;
            }
        }
        self.keys(cx, event);
        // Strip the stock Pop-to-root shortcut: the owner maintains the actual return path.
        let filtered;
        let forwarded = if let Event::Actions(actions) = event {
            filtered = Event::Actions(
                actions
                    .iter()
                    .filter_map(|a| a.as_widget_action())
                    .filter(|a| {
                        !matches!(
                            a.cast::<StackNavigationAction>(),
                            StackNavigationAction::Pop
                        )
                    })
                    .map(|a| Box::new(a.clone()) as Action)
                    .collect(),
            );
            &filtered
        } else {
            event
        };
        let overlay = self.current_overlay(cx);
        if overlay != OverlayKind::None {
            self.overlay(cx).handle_event(cx, forwarded, scope);
            // Saving can be retried while any presentation is open.
            self.view
                .widget(cx, path!(storage_notice))
                .handle_event(cx, forwarded, scope);
        } else if self.layout.kind == LayoutKind::Compact {
            if event.requires_visibility() {
                if let Some(id) = self.pages.last().copied() {
                    self.stack(cx)
                        .view_by_id(cx, id)
                        .handle_event(cx, forwarded, scope);
                } else {
                    self.view
                        .widget(cx, path!(root_view))
                        .handle_event(cx, forwarded, scope);
                }
            } else {
                self.view
                    .widget(cx, path!(compact_host))
                    .handle_event(cx, forwarded, scope);
            }
            self.view
                .widget(cx, path!(storage_notice))
                .handle_event(cx, forwarded, scope);
        } else {
            self.view
                .widget(
                    cx,
                    if self.layout.short_height {
                        path!(landscape)
                    } else {
                        path!(wide)
                    },
                )
                .handle_event(cx, forwarded, scope);
            self.view
                .widget(cx, path!(storage_notice))
                .handle_event(cx, forwarded, scope);
        }
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        // Host allocations, including hosted textures, are the only responsive input.
        let size = cx.turtle().rect().size;
        if size.x > 1.0 && size != self.dimensions {
            self.apply_layout(
                cx,
                LayoutDecision::decide(size.x, size.y, Some(self.layout)),
                size,
            );
        }
        self.populate(cx);
        if self.layout.short_height && self.current_overlay(cx) != OverlayKind::None {
            let all = std::mem::take(&mut self.view.children);
            self.view.children.extend(
                all.iter()
                    .filter(|(id, _)| *id == live_id!(overlays) || *id == live_id!(storage_notice))
                    .cloned(),
            );
            let result = self.view.draw_walk(cx, scope, walk);
            self.view.children = all;
            result
        } else {
            self.view.draw_walk(cx, scope, walk)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_draw::cx_draw::CxDraw;
    struct Canvas {
        pass: DrawPass,
        list: DrawList2d,
        overlay: Overlay,
    }
    fn draw(cx: &mut Cx, root: &WidgetRef, size: Vec2d, canvas: &mut Canvas) {
        let Canvas {
            pass,
            list,
            overlay,
        } = canvas;
        pass.set_size(cx, size);
        let event = DrawEvent {
            redraw_all: true,
            ..DrawEvent::default()
        };
        let mut drawing = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut drawing);
        cx2d.begin_pass(pass, Some(1.0));
        list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_overlay());
        overlay.begin(&mut cx2d);
        assert!(root
            .draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fill())
            .is_done());
        overlay.end(&mut cx2d);
        cx2d.end_pass_sized_turtle();
        list.end(&mut cx2d);
        cx2d.end_pass(pass);
    }
    #[test]
    fn cpu_layout_reaches_every_screen_at_the_acceptance_sizes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root=cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);makepad_wm_theme::apply(vm);crate::script_mod(vm);
            let v=script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* CalendarView{}});
            WidgetRef::script_from_value(vm,v)
        });
        {
            let mut view = root.borrow_mut::<CalendarView>().unwrap();
            view.seed_for_test(civil::from_ymd(2026, 9, 9));
            view.started = true;
        }
        let mut canvas = Canvas {
            pass: DrawPass::new(&mut cx),
            list: DrawList2d::new(&mut cx),
            overlay: cx.with_vm(|vm| Overlay::script_new(vm)),
        };
        draw(&mut cx, &root, dvec2(1240.0, 800.0), &mut canvas);
        let month_rect = root
            .widget(&mut cx, path!(wide.month_canvas))
            .area()
            .rect(&cx);
        assert_eq!(
            month_rect,
            crate::presentation::rect(221.0, 128.0, 1019.0, 672.0)
        );
        draw(&mut cx, &root, dvec2(402.0, 780.0), &mut canvas);
        let dates = root.widget(&mut cx, path!(month_dates)).area().rect(&cx);
        assert_eq!(dates, crate::presentation::rect(16.0, 128.0, 370.0, 264.0));
        assert_eq!(
            root.widget(&mut cx, path!(selected_agenda))
                .area()
                .rect(&cx)
                .size
                .y,
            288.0
        );
        for size in [
            dvec2(1240.0, 800.0),
            dvec2(874.0, 300.0),
            dvec2(402.0, 780.0),
        ] {
            draw(&mut cx, &root, size, &mut canvas);
            if size.y == 300.0 {
                assert_eq!(
                    root.widget(&mut cx, path!(landscape.week.body))
                        .area()
                        .rect(&cx)
                        .size
                        .y,
                    160.0
                );
                assert_eq!(
                    root.widget(&mut cx, path!(landscape.week.body.grid))
                        .area()
                        .rect(&cx)
                        .size
                        .y,
                    TIMELINE_CONTENT_HEIGHT
                );
            }
            for mode in [CalendarMode::Week, CalendarMode::Day, CalendarMode::Month] {
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    view.mode = mode;
                    view.sync_modes(&mut cx);
                }
                draw(&mut cx, &root, size, &mut canvas);
            }
            if size.x < 700.0 {
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    view.phone_mode = PhoneMode::Month;
                    view.sync_modes(&mut cx);
                }
                draw(&mut cx, &root, size, &mut canvas);
                assert_eq!(
                    root.widget(&mut cx, path!(month_dates))
                        .area()
                        .rect(&cx)
                        .size
                        .y,
                    588.0
                );
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    view.reduced_motion = true;
                    let day = view.selected;
                    view.open_day(&mut cx, day);
                }
                draw(&mut cx, &root, size, &mut canvas);
                assert!(
                    root.widget(&mut cx, path!(day_timeline))
                        .area()
                        .rect(&cx)
                        .size
                        .y
                        > 400.0
                );
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    view.back(&mut cx);
                    let day = view.selected;
                    view.open_agenda(&mut cx, day, true);
                }
                draw(&mut cx, &root, size, &mut canvas);
                assert!(!root
                    .widget(&mut cx, path!(agenda_page.upcoming_agenda))
                    .borrow::<CalendarAgendaList>()
                    .unwrap()
                    .frame
                    .events
                    .is_empty());
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    view.back(&mut cx);
                }
            }
            for kind in [
                OverlayKind::Detail,
                OverlayKind::Search,
                OverlayKind::Calendars,
                OverlayKind::Agenda,
                OverlayKind::Editor,
            ] {
                {
                    let mut view = root.borrow_mut::<CalendarView>().unwrap();
                    if kind == OverlayKind::Editor {
                        view.open_editor(&mut cx, None, None);
                    } else {
                        if kind == OverlayKind::Detail {
                            let key = occurrences_for_day(
                                view.document.as_ref().unwrap(),
                                view.selected,
                                true,
                                8,
                            )
                            .items[0]
                                .key;
                            view.selected_occ = Some(key);
                        }
                        if kind == OverlayKind::Search {
                            view.search_query = "Design".into();
                            view.sync_search_fields(&mut cx);
                        }
                        view.show_overlay(&mut cx, kind);
                    }
                }
                draw(&mut cx, &root, size, &mut canvas);
                let mut view = root.borrow_mut::<CalendarView>().unwrap();
                if let Some(mut host) = view.overlay(&mut cx).borrow_mut::<CalendarOverlayHost>() {
                    host.hide_immediately(&mut cx);
                }
                view.draft = None;
            }
        }
        assert!(
            cx.with_vm(|vm| vm.take_errors()).is_empty(),
            "all dynamic recipes must evaluate without errors"
        );
    }
    #[test]
    fn presentation_resize_preserves_one_draft_and_populates_all_modes() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            makepad_wm_theme::apply(vm);
            crate::script_mod(vm);
            let value = script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* CalendarView{}});
            let root = WidgetRef::script_from_value(vm, value);
            assert!(vm.take_errors().is_empty());
            root
        });
        let mut view = root.borrow_mut::<CalendarView>().unwrap();
        let day = civil::from_ymd(2026, 9, 9);
        view.seed_for_test(day);
        view.started = true;
        view.apply_layout(
            &mut cx,
            LayoutDecision::decide(1240.0, 800.0, None),
            dvec2(1240.0, 800.0),
        );
        view.populate(&mut cx);
        let original = encode(view.document.as_ref().unwrap());
        let id = view.document.as_ref().unwrap().events[0].id;
        view.open_editor(&mut cx, Some(id), None);
        let input = view.panel(&mut cx).text_input(&mut cx, path!(title_input));
        let uid = input.widget_uid();
        input.set_text(&mut cx, "Draft · 日本語");
        view.pull_fields(&mut cx);
        for size in [
            dvec2(402.0, 780.0),
            dvec2(874.0, 300.0),
            dvec2(1240.0, 800.0),
        ] {
            let layout = LayoutDecision::decide(size.x, size.y, Some(view.layout));
            view.apply_layout(&mut cx, layout, size);
            view.populate(&mut cx);
            let input = view.panel(&mut cx).text_input(&mut cx, path!(title_input));
            assert_eq!(input.widget_uid(), uid);
            assert_eq!(input.text(), "Draft · 日本語");
            assert_eq!(view.selected, day);
            assert_eq!(encode(view.document.as_ref().unwrap()), original);
            assert_eq!(view.current_overlay(&mut cx), OverlayKind::Editor);
        }
        view.draft = None;
        for mode in [CalendarMode::Month, CalendarMode::Week, CalendarMode::Day] {
            view.mode = mode;
            view.sync_modes(&mut cx);
            view.populate(&mut cx);
        }
        let query = "Design".to_string();
        view.search_query = query.clone();
        view.open_search(&mut cx);
        view.populate(&mut cx);
        assert_eq!(
            view.view
                .widget(&mut cx, path!(search_panel))
                .text_input(&mut cx, path!(query))
                .text(),
            query
        );
        let errors = cx.with_vm(|vm| vm.take_errors());
        assert!(
            errors.is_empty(),
            "presentation updates must evaluate without Splash errors: {errors:?}"
        );
    }
    #[test]
    fn midnight_defaults_keep_the_next_day_and_explicit_timeline_date() {
        let day = civil::from_ymd(2026, 9, 9);
        let clock = ClockSnapshot {
            today: day,
            minute: 23 * 60 + 50,
        };
        let event = default_new_event(clock, day, CAL_HOME, None);
        match event.timing {
            Timing::Timed { start, end } => {
                assert_eq!(start.day, day + 1);
                assert_eq!(start.minute, 0);
                assert_eq!(end.minute, 60);
            }
            _ => panic!("timed"),
        }
        assert_eq!(
            default_new_event(clock, day, CAL_HOME, Some(23 * 60))
                .timing
                .start_day(),
            day
        );
    }
    #[test]
    fn invalid_dates_and_times_remain_errors_even_when_fallback_timing_is_valid() {
        let day = civil::from_ymd(2026, 9, 9);
        let doc = crate::seed::seed(day);
        let value = doc.events[0].clone();
        let mut fields = DraftFields::from_event(&value);
        fields.start_time = "wrong".into();
        let mut draft = EventDraft {
            editing: Some(value.id),
            value: value.clone(),
            original: Some(value),
            end_was_edited: true,
        };
        let errors = apply_draft_fields(&mut draft, &fields, &doc.calendars);
        assert!(errors.start.is_some());
        assert!(!errors.is_empty());
        fields.start_time = "09:00".into();
        fields.end_date = "2026-02-30".into();
        assert!(apply_draft_fields(&mut draft, &fields, &doc.calendars)
            .end
            .is_some());
    }
    fn test_root() -> (Cx, WidgetRef, Canvas) {
        let mut cx=Cx::new(Box::new(|_,_|{}));
        cx.init_cx_os();
        let root=cx.with_vm(|vm|{
            makepad_widgets::script_mod(vm);makepad_wm_theme::apply(vm);crate::script_mod(vm);
            let v=script_eval!(vm,{use mod.prelude.widgets_internal.* use mod.widgets.* CalendarView{}});
            WidgetRef::script_from_value(vm,v)
        });
        {
            let mut view=root.borrow_mut::<CalendarView>().unwrap();
            view.seed_for_test(civil::from_ymd(2026,9,9));view.started=true;view.reduced_motion=true;
        }
        let canvas=Canvas{pass:DrawPass::new(&mut cx),list:DrawList2d::new(&mut cx),overlay:cx.with_vm(|vm|Overlay::script_new(vm))};
        (cx,root,canvas)
    }

    #[test]
    fn calendars_and_search_return_to_the_same_month_and_nested_day() {
        let (mut cx,root,mut canvas)=test_root();
        let size=dvec2(402.0,780.0);
        draw(&mut cx,&root,size,&mut canvas);
        for mode in [PhoneMode::Month,PhoneMode::MonthList] {
            {
                let mut view=root.borrow_mut::<CalendarView>().unwrap();
                view.phone_mode=mode;view.sync_modes(&mut cx);
                view.shift_period(&mut cx,1);
            }
            for search in [false,true] {
                let mut view=root.borrow_mut::<CalendarView>().unwrap();
                let before=(view.selected,view.displayed,view.month_anchor_dom);
                if search {view.open_search(&mut cx);} else {view.open_calendars(&mut cx);}
                view.back(&mut cx);
                assert_eq!(view.phone_mode,mode);
                assert_eq!((view.selected,view.displayed,view.month_anchor_dom),before);
                assert!(view.pages.is_empty());
            }
            draw(&mut cx,&root,size,&mut canvas);
            assert_eq!(root.widget(&mut cx,path!(month_dates)).area().rect(&cx).size.y,if mode==PhoneMode::Month{588.0}else{264.0});
        }
        let mut view=root.borrow_mut::<CalendarView>().unwrap();
        let day=view.selected;
        view.open_day(&mut cx,day);
        view.open_calendars(&mut cx);
        view.back(&mut cx);
        assert_eq!(view.pages,vec![live_id!(day_page)]);
        assert_eq!(view.phone_mode,PhoneMode::Day);
        view.select_day(&mut cx,day+1);
        assert_eq!(view.stack(&mut cx).view_by_id(&mut cx,live_id!(day_page))
            .label(&mut cx,path!(header.content.title_container.title)).text(),format_heading_compact_day(day+1));
        view.open_search(&mut cx);view.back(&mut cx);
        assert_eq!(view.selected,day+1);
        assert_eq!(view.pages,vec![live_id!(day_page)]);
    }

    #[test]
    fn breakpoint_toolbar_mini_month_and_calendar_rows_fit_their_allocations() {
        let (mut cx,root,mut canvas)=test_root();
        for width in [700.0,760.0,1040.0,1240.0] {
            draw(&mut cx,&root,dvec2(width,800.0),&mut canvas);
            let toolbar=root.widget(&mut cx,path!(wide.toolbar)).area().rect(&cx);
            let mut last=toolbar.pos.x;
            for path in [path!(add),path!(mode),path!(search),path!(previous),path!(today),path!(next)] {
                let r=root.widget(&mut cx,path!(wide.toolbar)).widget(&mut cx,path).area().rect(&cx);
                assert!(r.pos.x>=last-0.01,"overlapping toolbar at {width}: {r:?}");
                assert!(r.pos.x+r.size.x<=toolbar.pos.x+toolbar.size.x+0.01,"clipped toolbar at {width}: {r:?}");
                assert!(r.size.x>=44.0 && r.size.y>=44.0);
                last=r.pos.x+r.size.x;
            }
            let side=root.widget(&mut cx,path!(wide.sidebar)).area().rect(&cx);
            let mini=root.widget(&mut cx,path!(mini_month)).area().rect(&cx);
            assert!(mini.pos.x>=side.pos.x && mini.pos.x+mini.size.x<=side.pos.x+side.size.x);
        }
        draw(&mut cx,&root,dvec2(402.0,780.0),&mut canvas);
        root.borrow_mut::<CalendarView>().unwrap().open_calendars(&mut cx);
        draw(&mut cx,&root,dvec2(402.0,780.0),&mut canvas);
        for path in [path!(cal0),path!(cal1),path!(cal2),path!(cal3)] {
            let row=root.widget(&mut cx,path!(calendars_page.calendar_choices)).widget(&mut cx,path);
            let r=row.area().rect(&cx);
            let check=row.widget(&mut cx,path!(indicator)).area().rect(&cx);
            let name=row.widget(&mut cx,path!(calendar_name)).area().rect(&cx);
            assert_eq!(r.size.y,44.0);
            let center=check.pos.y+check.size.y*0.5;
            assert!(name.pos.y<=center && center<=name.pos.y+name.size.y,"{check:?} {name:?}");
            let text_walk=row.widget(&mut cx,path!(calendar_name)).walk(&mut cx);
            assert_eq!(text_walk.abs_pos.unwrap().x-check.pos.x-check.size.x,8.0);
        }
    }

    #[test]
    fn search_resize_preserves_editor_selection_undo_and_redo() {
        let (mut cx,root,mut canvas)=test_root();
        draw(&mut cx,&root,dvec2(1240.0,800.0),&mut canvas);
        root.borrow_mut::<CalendarView>().unwrap().open_search(&mut cx);
        draw(&mut cx,&root,dvec2(1240.0,800.0),&mut canvas);
        let editor=root.borrow::<CalendarView>().unwrap().query_editor.clone();
        let focus=|cx:&mut Cx|{
            editor.as_text_input().take_key_focus(cx);
            cx.widget_action(editor.widget_uid(),CalendarAction::None);
            cx.handle_actions();
        };
        focus(&mut cx);
        for input in ["Café"," 日本語"] {
            let actions=cx.capture_actions(|cx|editor.handle_event(cx,&Event::TextInput(TextInputEvent{input:input.into(),was_paste:true,..Default::default()}),&mut Scope::empty()));
            root.borrow_mut::<CalendarView>().unwrap().handle_actions(&mut cx,&actions);
        }
        assert_eq!(editor.as_text_input().text(),"Café 日本語");
        let selection=editor.as_text_input().selection();
        for size in [dvec2(402.0,780.0),dvec2(874.0,300.0),dvec2(1240.0,800.0),dvec2(402.0,780.0)] {
            draw(&mut cx,&root,size,&mut canvas);
            assert_eq!(root.borrow::<CalendarView>().unwrap().query_editor.widget_uid(),editor.widget_uid());
            let current=editor.as_text_input().selection();
            assert_eq!((current.anchor.index,current.cursor.index),(selection.anchor.index,selection.cursor.index));
            focus(&mut cx);
            for (shift, expected) in [(false,"Café"),(true,"Café 日本語")] {
                let actions=cx.capture_actions(|cx|editor.handle_event(cx,&Event::KeyDown(KeyEvent{key_code:KeyCode::KeyZ,modifiers:KeyModifiers{logo:true,control:true,shift,..Default::default()},..Default::default()}),&mut Scope::empty()));
                root.borrow_mut::<CalendarView>().unwrap().handle_actions(&mut cx,&actions);
                assert_eq!(editor.as_text_input().text(),expected);
            }
        }
    }

}
