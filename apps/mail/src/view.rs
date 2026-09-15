//! `MailView` owns the document, navigation, composer, and storage jail.

use crate::ai::{self, LoadState};
use crate::engine::{
    apply, layout_for, layout_needs_apply, list_rows, mailbox_count, parse_recipients, stack_step,
    Command, LayoutKind, LayoutMetrics, StackScreen, StackStep,
};
use crate::model::*;
use crate::seed;
use crate::storage::{self, SaveOutcome, SaveQueue, STORAGE_KEY};
use makepad_ai_services::wire::{ServiceCall, ToolResult};
use makepad_widgets::makepad_platform::storage::{
    StorageHandle, StorageRequestId, StorageResponse, StorageResult,
};
use makepad_widgets::*;

fn secondary_text_color(vm: &mut ScriptVm) -> ScriptValue {
    let theme = vm.module(id!(theme));
    let muted = vm.bx.heap.value(theme, id!(color_text_muted).into(), NoTrap);
    if !muted.is_nil() && !muted.is_err() {
        muted
    } else {
        // Stock themes without a WM palette predate color_text_muted. Resolve
        // their secondary role once; never override a host's explicit muted role.
        vm.bx.heap.value(theme, id!(color_text_meta).into(), NoTrap)
    }
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.MailViewBase = #(MailView::register_widget(vm))
    let muted_color = #(secondary_text_color(vm))

    let MailText = Label{
        width: Fill height: Fit
        padding: 0
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 12.75}
        }
    }
    let MailStrong = MailText{
        draw_text +: { text_style: theme.font_bold{font_size: 12.75} }
    }
    let MailMuted = MailText{
        draw_text +: { color: muted_color }
    }
    let MailPanel = GlassPanel{
        spacing: 0
        draw_bg +: {fallback_color: theme.color_inset tint_color: theme.color_inset}
    }
    let MailScope = mod.widgets.glass.GlassSegmented{
        draw_text +: {color: theme.color_text}
        draw_bg +: {
            color: uniform(theme.color_inset)
            pixel: fn() {return self.color}
        }
        draw_sel +: {fill_color: theme.color_bg_highlight}
    }
    let MailHit = ButtonFlat{
        margin: 0 padding: 0
        draw_bg +: {pixel: fn() {return vec4(0.0, 0.0, 0.0, 0.0)}}
    }
    let MailAction = mod.widgets.glass.GlassButton{
        width: 44 height: 44 padding: 0 margin: 0
        icon_walk: Walk{width: 20 height: 20}
        draw_text +: {color: theme.color_text}
        draw_glass +: {tint: theme.color_inset}
    }
    let MailSearch = TextInput{
        width: Fill height: 44
        empty_text: "Search"
        is_multiline: false
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 12.75}
        }
    }
    let MbBtn = Button{
        width: Fill height: 52
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 16 right: 12}
        icon_walk: Walk{width: 18 height: 18}
        draw_icon +: { color: theme.color_text }
        draw_text +: {
            color: theme.color_text
            text_style: theme.font_regular{font_size: 12.75}
        }
        draw_bg +: { color: #0000 }
    }
    let Chip = RoundedView{
        width: Fill height: Fit flow: Right spacing: 8 padding: Inset{left: 12 right: 12 top: 12 bottom: 12}
        align: Align{y: 0.5}
        draw_bg +: { color: theme.color_inset border_radius: 10 }
    }
    let SheetBtn = Button{
        width: Fill height: 52
        align: Align{x: 0.0 y: 0.5}
        padding: Inset{left: 16 right: 16}
        draw_text +: { color: theme.color_text text_style: theme.font_regular{font_size: 12.75} }
        draw_bg +: { color: #0000 }
    }
    let MessageRow = View{
        width: Fill height: 84
        flow: Overlay
        hit := MailHit{width: Fill height: Fill text: ""}
        sel := RoundedView{
            visible: false
            width: Fill height: Fill
            margin: Inset{left: 6 right: 6}
            draw_bg +: { color: theme.color_bg_highlight border_radius: 8 }
        }
        content := View{
            width: Fill height: Fill flow: Down
            padding: Inset{left: 28 right: 16 top: 9 bottom: 9}
            sender_line := View{
                flow: Right height: 17 clip_x: true
                sender := MailStrong{width: Fill max_lines: 1 text_overflow: Ellipsis}
                date := MailMuted{width: 68 max_lines: 1 text_overflow: Ellipsis}
            }
            subject_line := View{
                flow: Right height: 17 spacing: 6 clip_x: true
                subject := MailText{width: Fill max_lines: 1 text_overflow: Ellipsis}
                flag := View{
                    visible: false width: 14 height: 14
                    Icon{
                        width: 14 height: 14
                        icon_walk: Walk{width: 14 height: 14}
                        draw_icon +: { svg: crate_resource("self:resources/icons/flag.svg") color: theme.color_warning }
                    }
                }
                clip := View{
                    visible: false width: 14 height: 14
                    Icon{
                        width: 14 height: 14
                        icon_walk: Walk{width: 14 height: 14}
                        draw_icon +: { svg: crate_resource("self:resources/icons/attachment.svg") color: muted_color }
                    }
                }
            }
            preview := MailMuted{width: Fill height: 32 max_lines: 2 text_overflow: Ellipsis}
        }
        separator := SolidView{
            width: Fill height: 0.5 margin: Inset{left: 28}
            draw_bg.color: theme.color_bevel_outset_2
        }
        dot := RoundedView{
            visible: false
            width: 6 height: 6
            margin: Inset{left: 12 top: 20}
            draw_bg +: { color: theme.color_focus border_radius: 3 }
        }
    }
    let ReaderBlock = View{
        width: Fill height: Fit flow: Down spacing: 16
        padding: Inset{left: 16 right: 16 top: 24 bottom: 24}
        r_placeholder := MailMuted{text: "Select a message"}
        r_subject := MailStrong{
            draw_text +: { text_style: theme.font_bold{font_size: 16.5} }
        }
        sender := View{
            flow: Right height: Fit spacing: 12
            avatar := RoundedView{
                width: 40 height: 40
                align: Align{x: 0.5 y: 0.5}
                draw_bg +: { color: theme.color_focus border_radius: 20 }
                r_initials := MailStrong{width: Fit draw_text +: { color: theme.color_text }}
            }
            metadata := View{
                flow: Down width: Fill height: Fit spacing: 4
                r_from := MailStrong{}
                r_to := MailMuted{}
                r_cc := MailMuted{}
                r_date := MailMuted{}
            }
        }
        r_body := Markdown{
            width: Fill height: Fit padding: 0
            font_size: 12.75
            font_color: theme.color_text
            paragraph_spacing: 12
            use_code_block_widget: false
            use_math_widget: false
        }
        r_attachments := View{width: Fill flow: Down height: Fit spacing: 8
            chip0 := Chip{visible: false a0 := MailText{}}
            chip1 := Chip{visible: false a1 := MailText{}}
            chip2 := Chip{visible: false a2 := MailText{}}
            chip3 := Chip{visible: false a3 := MailText{}}
            chip4 := Chip{visible: false a4 := MailText{}}
            chip5 := Chip{visible: false a5 := MailText{}}
            chip6 := Chip{visible: false a6 := MailText{}}
            chip7 := Chip{visible: false a7 := MailText{}}
        }

    }

    mod.widgets.MailView = set_type_default() do mod.widgets.MailViewBase{
        width: Fill height: Fill
        show_bg: true
        draw_bg.color: theme.color_bg_app
        flow: Overlay

        loading := View{
            width: Fill height: Fill align: Align{x: 0.5 y: 0.5}
            MailMuted{text: "Loading mail…"}
        }
        error_page := View{
            visible: false width: Fill height: Fill flow: Down spacing: 16
            align: Align{x: 0.5 y: 0.5} padding: 24
            error_text := MailText{text: "Could not load mail"}
            retry := Button{text: "Retry" height: 44 width: 160}
        }

        wide := View{
            visible: false width: Fill height: Fill flow: Down
            wide_toolbar := MailPanel{
                height: 56 padding: 6 flow: Right spacing: 8 align: Align{y: 0.5}
                wide_title := MailStrong{text: "Mail" width: 220 draw_text +: { text_style: theme.font_bold{font_size: 12.75} }}
                wide_mailbox_btn := Button{visible: false text: "Inbox" height: 44 margin: 0}
                wide_compose := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/compose.svg") color: theme.color_text}}
                wide_reply := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/reply.svg") color: theme.color_text}}
                wide_forward := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/forward.svg") color: theme.color_text}}
                wide_archive := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/archive.svg") color: theme.color_text}}
                wide_delete := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/delete.svg") color: theme.color_text}}
                wide_flag := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/flag.svg") color: theme.color_warning}}
                wide_more := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/more.svg") color: theme.color_text}}
                View{width: Fill}
                wide_search := MailSearch{width: 240 height: 36 margin: 0}
            }
            columns := View{
                flow: Right height: Fill spacing: 0
                sidebar := ScrollYView{
                    width: 220
                    flow: Down padding: 12 spacing: 8
                    MailMuted{text: "Favorites" draw_text +: { text_style: theme.font_regular{font_size: 9} }}
                    side_inbox := MbBtn{text: "Inbox" draw_icon.svg: crate_resource("self:resources/icons/inbox.svg")}
                    side_vips := MbBtn{text: "VIPs" draw_icon.svg: crate_resource("self:resources/icons/vip.svg")}
                    side_flagged := MbBtn{text: "Flagged" draw_icon.svg: crate_resource("self:resources/icons/flag.svg")}
                    MailMuted{text: "Mailboxes" margin: Inset{top: 12} draw_text +: { text_style: theme.font_regular{font_size: 9} }}
                    side_drafts := MbBtn{text: "Drafts" draw_icon.svg: crate_resource("self:resources/icons/drafts.svg")}
                    side_sent := MbBtn{text: "Sent" draw_icon.svg: crate_resource("self:resources/icons/sent.svg")}
                    side_archive := MbBtn{text: "Archive" draw_icon.svg: crate_resource("self:resources/icons/archive.svg")}
                    side_junk := MbBtn{text: "Junk" draw_icon.svg: crate_resource("self:resources/icons/junk.svg")}
                    side_trash := MbBtn{text: "Trash" draw_icon.svg: crate_resource("self:resources/icons/trash.svg")}
                    side_projects := MbBtn{text: "Projects" draw_icon.svg: crate_resource("self:resources/icons/projects.svg")}
                    side_travel := MbBtn{text: "Travel" draw_icon.svg: crate_resource("self:resources/icons/travel.svg")}
                }
                divider_a := SolidView{width: 1 draw_bg.color: theme.color_bevel_outset_2}
                list_column := View{
                    show_bg: true draw_bg.color: theme.color_inset
                    width: 360 flow: Down
                    heading := View{
                        height: 64 flow: Down padding: Inset{left: 16 right: 16 top: 12}
                        wide_heading := MailStrong{text: "Inbox"}
                        wide_counts := MailMuted{}
                    }
                    wide_scope := MailScope{
                        visible: false height: 44
                        labels: ["This Mailbox", "All Mail"]
                    }
                    wide_list := PortalList{
                        width: Fill height: Fill
                        Message := MessageRow{}
                    }
                    wide_empty := View{
                        visible: false width: Fill height: Fill flow: Down
                        align: Align{x: 0.5 y: 0.5} spacing: 8
                        wide_empty_title := MailStrong{text: "No messages"}
                        wide_empty_sub := MailMuted{}
                        clear_search := Button{visible: false text: "Clear Search" height: 44}
                    }
                    wide_status_bar := View{height: 24 flow: Right
                        wide_status := MailMuted{height: Fill padding: Inset{left: 16} text: "Demo mail · Saved"}
                    }
                }
                divider_b := SolidView{width: 1 draw_bg.color: theme.color_bevel_outset_2}
                wide_reader := ScrollYView{width: Fill height: Fill
                    reader_content := ReaderBlock{}
                }
            }
        }

        phone_shell := View{
            visible: false width: Fill height: Fill
        phone := StackNavigation{
            root_view +: {
                flow: Down
                show_bg: true
                draw_bg.color: theme.color_bg_app
                nav := View{height: 52}
                title := MailStrong{
                    height: 52 text: "Mailboxes" padding: Inset{left: 20}
                    draw_text +: { text_style: theme.font_bold{font_size: 25.5} }
                }
                mailbox_body := ScrollYView{
                    height: Fill flow: Down padding: Inset{left: 16 right: 16} spacing: 12
                    favorites := RoundedView{
                        flow: Down height: Fit
                        draw_bg +: { color: theme.color_inset border_radius: 16 }
                        mb_inbox := MbBtn{text: "Inbox" height: 52 draw_icon.svg: crate_resource("self:resources/icons/inbox.svg")}
                        mb_vips := MbBtn{text: "VIPs" height: 52 draw_icon.svg: crate_resource("self:resources/icons/vip.svg")}
                        mb_flagged := MbBtn{text: "Flagged" height: 52 draw_icon.svg: crate_resource("self:resources/icons/flag.svg")}
                    }
                    local_mail := RoundedView{
                        flow: Down height: Fit
                        draw_bg +: { color: theme.color_inset border_radius: 16 }
                        mb_drafts := MbBtn{text: "Drafts" draw_icon.svg: crate_resource("self:resources/icons/drafts.svg")}
                        mb_sent := MbBtn{text: "Sent" draw_icon.svg: crate_resource("self:resources/icons/sent.svg")}
                        mb_archive := MbBtn{text: "Archive" draw_icon.svg: crate_resource("self:resources/icons/archive.svg")}
                        mb_junk := MbBtn{text: "Junk" draw_icon.svg: crate_resource("self:resources/icons/junk.svg")}
                        mb_trash := MbBtn{text: "Trash" draw_icon.svg: crate_resource("self:resources/icons/trash.svg")}
                        mb_projects := MbBtn{text: "Projects" draw_icon.svg: crate_resource("self:resources/icons/projects.svg")}
                        mb_travel := MbBtn{text: "Travel" draw_icon.svg: crate_resource("self:resources/icons/travel.svg")}
                    }
                }
                actions := MailPanel{
                    height: 52 margin: Inset{left: 16 right: 16 bottom: 8}
                    flow: Right padding: 4 align: Align{y: 0.5}
                    phone_status_mb := MailMuted{width: Fill text: "Demo mail · Saved"}
                    phone_compose_mb := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/compose.svg") color: theme.color_text}}
                }
            }
            messages := StackNavigationView{
                header +: {visible: false height: 0}
                body +: {
                    margin: Inset{top: 0}
                    flow: Down
                    show_bg: true
                    draw_bg.color: theme.color_bg_app
                    list_nav := View{
                        height: 52 flow: Right padding: Inset{left: 4} align: Align{y: 0.5}
                        list_back := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/back.svg") color: theme.color_text}}
                    }
                    list_title := MailStrong{
                        height: 52 padding: Inset{left: 20}
                        draw_text +: { text_style: theme.font_bold{font_size: 25.5} }
                    }
                    phone_search := MailSearch{margin: Inset{left: 16 right: 16} height: 44}
                    phone_scope := MailScope{
                        visible: false height: 44 margin: Inset{left: 16 right: 16}
                        labels: ["This Mailbox", "All Mail"]
                    }
                    phone_list := PortalList{
                        width: Fill height: Fill
                        Message := MessageRow{height: 104}
                    }
                    phone_empty := View{
                        visible: false width: Fill height: Fill flow: Down
                        align: Align{x: 0.5 y: 0.5} spacing: 8 padding: 16
                        phone_empty_title := MailStrong{text: "No messages"}
                        phone_empty_sub := MailMuted{}
                        phone_clear_search := Button{visible: false text: "Clear Search" height: 44}
                    }
                    list_actions := MailPanel{
                        height: 52 margin: Inset{left: 16 right: 16 bottom: 8}
                        flow: Right padding: 4 align: Align{y: 0.5} spacing: 8
                        phone_unread := Button{text: "Unread" height: 44}
                        phone_status_list := MailMuted{width: Fill}
                        phone_compose_list := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/compose.svg") color: theme.color_text}}
                    }
                }
            }
            message := StackNavigationView{
                header +: {visible: false height: 0}
                body +: {
                    margin: Inset{top: 0}
                    flow: Down
                    show_bg: true
                    draw_bg.color: theme.color_bg_app
                    read_nav := View{
                        height: 52 flow: Right padding: Inset{left: 4} align: Align{y: 0.5}
                        read_back := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/back.svg") color: theme.color_text}}
                        read_title := MailStrong{width: Fill}
                    }
                    phone_reader := ScrollYView{height: Fill
                        reader_content := ReaderBlock{}
                    }
                    read_actions := MailPanel{
                        height: 52 margin: Inset{left: 16 right: 16 bottom: 8}
                        flow: Right padding: 4 align: Align{y: 0.5} spacing: 8
                        phone_archive := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/archive.svg") color: theme.color_text}}
                        phone_move := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/projects.svg") color: theme.color_text}}
                        phone_delete := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/delete.svg") color: theme.color_error}}
                        phone_reply := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/reply.svg") color: theme.color_text}}
                        View{width: Fill}
                        phone_compose_read := MailAction{draw_icon +: {svg: crate_resource("self:resources/icons/compose.svg") color: theme.color_text}}
                    }
                }
            }
        }
        }

        save_error_bar := MailPanel{
            visible: false width: Fill height: 52 flow: Right padding: 4
            save_error_text := MailText{width: Fill text: "Save failed"}
            save_retry := Button{text: "Retry Save" width: 120 height: 44}
        }

        compose := Modal{
            can_dismiss: false
            bg_view +: { draw_bg.color: #00000038 }
            content +: {
                compose_sheet := MailPanel{
                    width: 680 height: 600 padding: 0 flow: Down
                    draw_bg +: { corner_radius: 24 }
                    compose_header := View{
                        height: 56 flow: Right padding: Inset{left: 8 right: 8} align: Align{y: 0.5}
                        compose_cancel := Button{text: "Cancel" height: 44}
                        compose_title := MailStrong{width: Fill text: "New Message"}
                        compose_send := Button{text: "Send" height: 44}
                    }
                    form := ScrollYView{
                        height: Fill flow: Down padding: Inset{left: 16 right: 16}
                        compose_to := TextInput{height: 44 empty_text: "To" is_multiline: false}
                        compose_to_err := MailText{visible: false draw_text.color: theme.color_error}
                        compose_cc := TextInput{height: 44 empty_text: "Cc" is_multiline: false}
                        compose_cc_err := MailText{visible: false draw_text.color: theme.color_error}
                        compose_from := MailMuted{height: 44}
                        compose_subject := TextInput{height: 44 empty_text: "Subject" is_multiline: false}
                        compose_body := TextInput{
                            height: 312
                            is_multiline: true
                            submit_on_enter: false
                            empty_text: "Message"
                        }
                        compose_attachments := View{width: Fill height: Fit flow: Down spacing: 8
                            fchip0 := Chip{visible: false f0 := MailText{}}
                            fchip1 := Chip{visible: false f1 := MailText{}}
                            fchip2 := Chip{visible: false f2 := MailText{}}
                            fchip3 := Chip{visible: false f3 := MailText{}}
            fchip4 := Chip{visible: false f4 := MailText{}}
            fchip5 := Chip{visible: false f5 := MailText{}}
            fchip6 := Chip{visible: false f6 := MailText{}}
            fchip7 := Chip{visible: false f7 := MailText{}}
                        }
                    }
                    compose_footer := View{height: 44 flow: Right
                        compose_status := MailMuted{height: Fill padding: Inset{left: 16} text: "Saved locally"}
                        compose_retry := Button{visible: false text: "Retry Save" width: 120 height: 44}
                    }
                }
            }
        }

        action_sheet := Modal{
            can_dismiss: true
            bg_view +: { draw_bg.color: #00000038 }
            content +: {
                panel := MailPanel{
                    width: 370 height: 500 padding: 8 flow: Down
                    choices := ScrollYView{width: Fill height: Fill flow: Down
                    sheet_title := MailStrong{padding: 12}
                    sheet0 := SheetBtn{visible: false}
                    sheet1 := SheetBtn{visible: false}
                    sheet2 := SheetBtn{visible: false}
                    sheet3 := SheetBtn{visible: false}
                    sheet4 := SheetBtn{visible: false}
                    sheet5 := SheetBtn{visible: false}
                    sheet6 := SheetBtn{visible: false}
                    sheet7 := SheetBtn{visible: false}
                    sheet8 := SheetBtn{visible: false}
                    sheet9 := SheetBtn{visible: false}
                    sheet_cancel := SheetBtn{text: "Cancel"}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SheetKind {
    #[default]
    None,
    MessageActions,
    MoveTo,
    DraftCancel,
    MailboxPicker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingDismiss {
    Send { revision: u64 },
    SaveDraft { revision: u64 },
}

#[derive(Script, ScriptHook, Widget)]
pub struct MailView {
    #[deref]
    view: View,
    #[rust]
    started: bool,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    load_id: Option<StorageRequestId>,
    #[rust]
    doc: Option<MailDocument>,
    #[rust]
    load: LoadState,
    #[rust]
    error: String,
    #[rust]
    ui: UiState,
    #[rust]
    rows: Vec<MessageRow>,
    #[rust]
    save: SaveQueue,
    #[rust]
    status: String,
    #[rust]
    bound_composer: Option<MessageId>,
    #[rust]
    sheet: SheetKind,
    #[rust]
    sheet_actions: Vec<SheetChoice>,
    #[rust]
    pending_dismiss: Option<PendingDismiss>,
    #[rust]
    closing: bool,
    #[rust]
    quit_ready: bool,
    #[rust]
    last_metrics: Option<LayoutMetrics>,
    #[rust]
    last_size: Option<DVec2>,
    #[rust]
    last_ready: Option<(bool, bool)>,
    #[rust]
    save_failed: bool,
    #[rust]
    editor_error: Option<String>,
    #[rust]
    compact: bool,
    #[rust]
    to_error: String,
    #[rust]
    cc_error: String,
    #[rust]
    keep_reader: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SheetChoice {
    Reply,
    Forward,
    Flag,
    Unflag,
    MarkRead,
    MarkUnread,
    MoveTo,
    MoveFolder(Folder),
    SaveDraft,
    DiscardDraft,
    KeepEditing,
    Mailbox(Mailbox),
}

impl MailView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    pub fn query(&self) -> &str {
        &self.ui.list.query
    }

    pub fn set_query_for_test(&mut self, query: &str) {
        self.ui.list.query = query.to_string();
    }

    pub fn ai_summary(&self) -> String {
        match self.load {
            LoadState::Loading => "Mail is loading".into(),
            LoadState::Error => format!("Mail failed to load: {}", self.error),
            LoadState::Ready => {
                let unread = self
                    .doc
                    .as_ref()
                    .map(|d| mailbox_count(d, Mailbox::Folder(Folder::Inbox)))
                    .unwrap_or(0);
                format!("Mail · {} · {unread} unread in Inbox", self.ui.list.mailbox.label())
            }
        }
    }

    pub fn ai_answer(&self, call: &ServiceCall) -> ToolResult {
        ai::answer(self.doc.as_ref(), self.load, call)
    }

    pub fn request_close(&mut self, cx: &mut Cx) -> bool {
        if self.pending_dismiss.is_none() && self.save_composer_fields(cx).is_err() {
            return false;
        }
        if !self.save.pending() {
            return true;
        }
        self.closing = true;
        self.flush_save(cx);
        false
    }

    pub fn should_quit(&self) -> bool {
        self.quit_ready
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.status = "Demo mail · Saved".into();
        if let Some(storage) = self.storage.as_ref() {
            self.load_id = Some(storage.get(cx, STORAGE_KEY));
            self.load = LoadState::Loading;
        } else {
            self.fail_load(cx, "Mail storage is unavailable".into());
        }
        self.sync_chrome(cx);
    }

    fn install_seed(&mut self, cx: &mut Cx) {
        let secs = Cx::time_now().max(0.0) as i64;
        let doc = seed::generate(anchor_from_unix(secs));
        self.install_doc(cx, doc, true);
    }

    fn install_doc(&mut self, cx: &mut Cx, doc: MailDocument, seed_now: bool) {
        self.doc = Some(doc);
        self.load = LoadState::Ready;
        self.error.clear();
        self.rebuild_rows();
        if seed_now {
            self.save.note_dirty();
            self.flush_save(cx);
        }
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn rebuild_rows(&mut self) {
        let previous = self.rows.clone();
        if let Some(doc) = &self.doc {
            self.rows = list_rows(doc, &self.ui.list);
        } else {
            self.rows.clear();
        }
        crate::engine::reconcile_anchor(&mut self.ui.list.scroll, &previous, &self.rows);
    }

    fn now() -> i64 {
        Cx::time_now().max(0.0) as i64
    }

    fn mutate(&mut self, cx: &mut Cx, command: Command) -> Result<Mutation, MailError> {
        self.capture_scroll(cx);
        let doc = self.doc.as_mut().ok_or(MailError::InvalidDocument("mail is not ready".into()))?;
        let mutation = match apply(doc, command, Self::now()) {
            Ok(mutation) => mutation,
            Err(err) => {
                self.status = err.to_string();
                self.sync_status(cx);
                return Err(err);
            }
        };
        self.save.note_dirty();
        self.flush_save(cx);
        self.rebuild_rows();
        self.restore_scroll(cx);
        self.sync_chrome(cx);
        self.view.redraw(cx);
        Ok(mutation)
    }

    fn flush_save(&mut self, cx: &mut Cx) {
        let Some(rev) = self.save.take_submit() else { return };
        let Some(doc) = &self.doc else { return };
        match storage::encode_document(doc) {
            Ok(bytes) => {
                if let Some(storage) = self.storage.as_ref() {
                    let id = storage.set(cx, STORAGE_KEY, bytes);
                    self.save.begin(id.0, rev);
                    self.save_failed = false;
                    self.status = "Saving…".into();
                    self.sync_status(cx);
                }
            }
            Err(err) => {
                self.save_failed = true;
                self.status = err.to_string();
                self.sync_status(cx);
            }
        }
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        for response in responses {
            if self.load_id == Some(response.request_id) {
                self.load_id = None;
                match &response.result {
                    Ok(StorageResult::Value(None)) => self.install_seed(cx),
                    Ok(StorageResult::Value(Some(bytes))) => match storage::decode_document(bytes) {
                        Ok(doc) => self.install_doc(cx, doc, false),
                        Err(err) => self.fail_load(cx, err.to_string()),
                    },
                    Err(err) => self.fail_load(cx, err.to_string()),
                    _ => self.fail_load(cx, "unexpected storage result".into()),
                }
            }
            match self.save.on_response(response.request_id.0, matches!(response.result, Ok(StorageResult::Unit))) {
                SaveOutcome::Ignore => {}
                SaveOutcome::Ack { revision, submit_queued } => {
                    self.status = "Demo mail · Saved".into();
                    if let Some(pending) = self.pending_dismiss {
                        let wait = match pending {
                            PendingDismiss::Send { revision } | PendingDismiss::SaveDraft { revision } => revision,
                        };
                        if revision >= wait && !self.save.pending() && self.editor_error.is_none() {
                            self.close_compose(cx);
                            if matches!(pending, PendingDismiss::Send { .. }) {
                                self.status = "Saved to Sent · Demo".into();
                            }
                            self.pending_dismiss = None;
                        }
                    }
                    self.save.queued = submit_queued;
                    self.flush_save(cx);
                    if self.closing && !self.save.pending() && self.editor_error.is_none() {
                        self.quit_ready = true;
                    }
                    self.sync_status(cx);
                }
                SaveOutcome::Fail { retry, .. } => {
                    self.save_failed = true;
                    self.status = "Save failed · Retry Save".into();
                    self.save.queued = Some(retry);
                    self.sync_status(cx);
                    self.quit_ready = false;
                }
            }
        }
    }

    fn fail_load(&mut self, cx: &mut Cx, error: String) {
        self.load = LoadState::Error;
        self.error = error;
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn update_layout(&mut self, cx: &mut Cx, size: DVec2) {
        if size.x <= 1.0 { return; }
        let metrics = layout_for(size.x, size.y);
        let ready = self.load == LoadState::Ready;
        let error = self.load == LoadState::Error;
        let last_kind = self.last_metrics.map(|m| (m.kind, m.short));
        let last_size = self.last_size.map(|s| (s.x.round() as u32, s.y.round() as u32));
        if layout_needs_apply(last_kind, last_size, self.last_ready, metrics, size.x, size.y, ready, error) {
            self.capture_scroll(cx);
            self.last_size = Some(size);
            self.last_ready = Some((ready, error));
            self.apply_layout(cx, metrics);
            self.restore_scroll(cx);
        }
        if self.compact { self.sync_stack(cx); }
    }

    fn apply_layout(&mut self, cx: &mut Cx, metrics: LayoutMetrics) {
        self.compact = metrics.kind == LayoutKind::Compact;
        self.view.widget(cx, ids!(sidebar)).set_visible(cx, metrics.show_sidebar);
        self.view.widget(cx, ids!(wide_mailbox_btn)).set_visible(cx, !self.compact && !metrics.show_sidebar);
        let mut list_col = self.view.widget(cx, ids!(list_column));
        script_apply_eval!(cx, list_col, { width: #(metrics.list_w) });
        let mut sheet = self.view.widget(cx, ids!(compose_sheet));
        script_apply_eval!(cx, sheet, { width: #(metrics.compose_w) height: #(metrics.compose_h) });
        self.view.widget(cx, ids!(divider_a)).set_visible(cx, metrics.show_sidebar);
        self.view.widget(cx, ids!(wide_title)).set_visible(cx, metrics.show_sidebar);
        for id in [ids!(wide_forward), ids!(wide_flag)] {
            self.view.widget(cx, id).set_visible(cx, !metrics.collapse_extra_actions);
        }
        let mut toolbar = self.view.widget(cx, ids!(wide_toolbar));
        let vertical = if metrics.short {2.0} else {6.0};
        script_apply_eval!(cx, toolbar, {height: #(metrics.toolbar_h)
            padding: mod.prelude.widgets.Inset{left: 6 right: 6 top: #(vertical) bottom: #(vertical)}});
        let mut selector = self.view.widget(cx, ids!(wide_mailbox_btn));
        script_apply_eval!(cx, selector, {width: 140});
        let mut heading = self.view.widget(cx, ids!(heading));
        script_apply_eval!(cx, heading, {height: #(metrics.list_header_h)});
        self.view.widget(cx, ids!(wide_counts)).set_visible(cx, !metrics.short);
        self.view.widget(cx, ids!(wide_status_bar)).set_visible(cx, !metrics.short);
        let mut header = self.view.widget(cx, ids!(compose_header));
        script_apply_eval!(cx, header, {height: #(if metrics.short {48.0} else {56.0})});
        let mut body = self.view.widget(cx, ids!(compose_body));
        script_apply_eval!(cx, body, {height: #(if metrics.short {180.0} else {312.0})});
        for mb in Mailbox::ALL {
            let mut button = self.view.widget(cx, side_id(mb).unwrap());
            script_apply_eval!(cx, button, {height: 36});
        }
        for (id, compact_reader) in [(ids!(wide_reader), false), (ids!(phone_reader), true)] {
            let root = self.view.widget(cx, id);
            let mut content = root.widget(cx, ids!(reader_content));
            let horizontal = if compact_reader {16.0} else {28.0};
            let width = if compact_reader {metrics.list_w} else {metrics.reader_w.min(656.0)};
            script_apply_eval!(cx, content, {width: #(width) padding: mod.prelude.widgets.Inset{left: #(horizontal) right: #(horizontal) top: 24 bottom: 24}});
            let mut body = root.widget(cx, ids!(r_body));
            script_apply_eval!(cx, body, {font_size: #(if compact_reader {12.75} else {11.25})});
        }
        self.last_metrics = Some(metrics);
        self.size_action_sheet(cx);
        if self.compact {
            self.sync_stack(cx);
        }
        self.sync_chrome(cx);
    }

    fn size_action_sheet(&mut self, cx: &mut Cx) {
        let size = self.last_size.unwrap_or(dvec2(1240.0, 800.0));
        let picker = self.sheet == SheetKind::MailboxPicker;
        let width = (size.x - 16.0).max(0.0).min(if picker {280.0} else {370.0});
        let content_height = self.sheet_actions.len() as f64 * 52.0 + 116.0;
        let height = (size.y - 16.0).max(0.0).min(content_height).min(if picker {244.0} else {600.0});
        let mut panel = self.view.widget(cx, ids!(panel));
        script_apply_eval!(cx, panel, {width: #(width) height: #(height)});
    }

    fn capture_scroll(&mut self, cx: &Cx) {
        if self.compact && !matches!(self.ui.history.last(), Some(Route::Messages)) { return; }
        let id = if self.compact {ids!(phone_list)} else {ids!(wide_list)};
        if let Some(list) = self.view.portal_list(cx, id).borrow() {
            if let Some(row) = self.rows.get(list.first_id()) {
                self.ui.list.scroll = ScrollAnchor {first_message: Some(row.id), offset_points: list.first_scroll()};
            }
        }
    }

    fn restore_scroll(&self, cx: &Cx) {
        let first = self.ui.list.scroll.first_message
            .and_then(|id| self.rows.iter().position(|row| row.id == id)).unwrap_or(0);
        for id in [ids!(wide_list), ids!(phone_list)] {
            if let Some(mut list) = self.view.portal_list(cx, id).borrow_mut() {
                if list.first_id() != first || list.first_scroll() != self.ui.list.scroll.offset_points {
                    list.set_first_id_and_scroll(first, self.ui.list.scroll.offset_points);
                }
            }
        }
    }

    fn sync_stack(&mut self, cx: &mut Cx) {
        let nav = self.view.stack_navigation(cx, ids!(phone));
        if nav.is_transitioning() {
            return;
        }
        let current = match nav.current_view() {
            None => StackScreen::Root,
            Some(id) if id == live_id!(messages) => StackScreen::Messages,
            Some(id) if id == live_id!(message) => StackScreen::Message,
            Some(_) => StackScreen::Root,
        };
        let desired = self.ui.history.last().copied().unwrap_or(Route::Mailboxes);
        match stack_step(current, desired) {
            StackStep::Stay => {}
            StackStep::PopRoot => nav.pop_to_root(cx),
            StackStep::PopMessages => nav.pop_to_view(cx, live_id!(messages)),
            StackStep::PushMessages => nav.push(cx, live_id!(messages)),
            StackStep::PushMessage => nav.push(cx, live_id!(message)),
        }
    }

    fn sync_status(&mut self, cx: &mut Cx) {
        self.view.widget(cx, ids!(save_error_bar)).set_visible(cx, self.save_failed);
        self.view.widget(cx, ids!(compose_retry)).set_visible(cx, self.save_failed);
        self.view.label(cx, ids!(save_error_text)).set_text(cx, &self.status);
        self.view.label(cx, ids!(wide_status)).set_text(cx, &self.status);
        self.view.label(cx, ids!(phone_status_mb)).set_text(cx, &self.status);
        self.view.label(cx, ids!(phone_status_list)).set_text(cx, &self.status);
        self.view.label(cx, ids!(compose_status)).set_text(cx, self.editor_error.as_deref().unwrap_or(&self.status));
        if !self.error.is_empty() {
            self.view.label(cx, ids!(error_text)).set_text(cx, &self.error);
        }
    }

    fn sync_chrome(&mut self, cx: &mut Cx) {
        let ready = self.load == LoadState::Ready;
        self.view.widget(cx, ids!(loading)).set_visible(cx, self.load == LoadState::Loading);
        self.view.widget(cx, ids!(error_page)).set_visible(cx, self.load == LoadState::Error);
        self.view.widget(cx, ids!(wide)).set_visible(cx, ready && !self.compact);
        self.view.widget(cx, ids!(phone_shell)).set_visible(cx, ready && self.compact);
        self.sync_status(cx);
        for id in [ids!(wide_search), ids!(phone_search)] {
            let input = self.view.text_input(cx, id);
            if input.text() != self.ui.list.query { input.set_text(cx, &self.ui.list.query); }
        }
        for id in [ids!(wide_scope), ids!(phone_scope)] {
            let scope = self.view.glass_segmented(cx, id);
            let selected = usize::from(self.ui.list.scope == SearchScope::AllMail);
            if scope.borrow().is_some_and(|scope| scope.selected != selected) {
                scope.set_selected(cx, selected);
            }
        }
        let sending = matches!(self.pending_dismiss, Some(PendingDismiss::Send { .. }));
        for id in [ids!(compose_to), ids!(compose_cc), ids!(compose_subject), ids!(compose_body)] {
            let input = self.view.text_input(cx, id);
            if input.is_read_only() != sending {
                input.set_is_read_only(cx, sending);
            }
        }
        for id in [ids!(compose_to), ids!(compose_cc), ids!(compose_subject), ids!(compose_body), ids!(compose_send), ids!(compose_cancel)] {
            self.view.widget(cx, id).set_disabled(cx, sending);
        }
        let mailbox = self.ui.list.mailbox;
        self.view.label(cx, ids!(wide_heading)).set_text(cx, mailbox.label());
        self.view.label(cx, ids!(list_title)).set_text(cx, mailbox.label());
        self.view.button(cx, ids!(wide_mailbox_btn)).set_text(cx, mailbox.label());
        let searching = !self.ui.list.query.trim().is_empty();
        self.view.widget(cx, ids!(wide_scope)).set_visible(cx, searching);
        self.view.widget(cx, ids!(phone_scope)).set_visible(cx, searching);
        let empty = self.rows.is_empty() && self.load == LoadState::Ready;
        self.view.widget(cx, ids!(wide_empty)).set_visible(cx, empty);
        self.view.widget(cx, ids!(wide_list)).set_visible(cx, !empty);
        self.view.widget(cx, ids!(phone_list)).set_visible(cx, !empty);
        self.view.widget(cx, ids!(phone_empty)).set_visible(cx, empty);
        self.view.label(cx, ids!(phone_empty_title)).set_text(cx, if searching {"No results"} else {"No messages"});
        self.view.label(cx, ids!(phone_empty_sub)).set_text(cx, if searching {&self.ui.list.query} else {mailbox.label()});
        self.view.widget(cx, ids!(phone_clear_search)).set_visible(cx, searching);
        if searching {
            self.view.label(cx, ids!(wide_empty_title)).set_text(cx, &format!("No results for ‘{}’", self.ui.list.query.trim()));
            self.view.widget(cx, ids!(clear_search)).set_visible(cx, true);
        } else {
            self.view.label(cx, ids!(wide_empty_title)).set_text(cx, "No messages");
            self.view.label(cx, ids!(wide_empty_sub)).set_text(cx, mailbox.label());
            self.view.widget(cx, ids!(clear_search)).set_visible(cx, false);
        }
        if let Some(doc) = &self.doc {
            let total = self.rows.len();
            let unread = mailbox_count(doc, mailbox);
            self.view.label(cx, ids!(wide_counts)).set_text(
                cx,
                &format!("{total} messages · {unread} unread"),
            );
            for mb in Mailbox::ALL {
                let count = mailbox_count(doc, mb);
                let label = if count == 0 {
                    mb.label().to_string()
                } else {
                    format!("{}  {count}", mb.label())
                };
                if let Some(id) = side_id(mb) {
                    self.view.button(cx, id).set_text(cx, &label);
                }
                if let Some(id) = mb_id(mb) {
                    self.view.button(cx, id).set_text(cx, &label);
                }
            }
            self.view.label(cx, ids!(compose_from)).set_text(cx, &format!("From: {}", doc.me.display()));
        }
        let has_sel = self.ui.selected.is_some();
        self.view.widget(cx, ids!(wide_reply)).set_disabled(cx, !has_sel);
        self.view.widget(cx, ids!(wide_forward)).set_disabled(cx, !has_sel);
        self.view.widget(cx, ids!(wide_archive)).set_disabled(cx, !has_sel);
        let can_delete = self.ui.selected.and_then(|id| self.doc.as_ref()?.message(id)).is_some_and(|m| m.folder != Folder::Trash);
        self.view.widget(cx, ids!(wide_delete)).set_disabled(cx, !can_delete);
        self.view.widget(cx, ids!(phone_delete)).set_disabled(cx, !can_delete);
        self.view.widget(cx, ids!(wide_flag)).set_disabled(cx, !has_sel);
        self.view.widget(cx, ids!(wide_more)).set_disabled(cx, !has_sel);
        self.bind_reader(cx);
    }

    fn bind_reader(&mut self, cx: &mut Cx) {
        self.bind_reader_root(cx, ids!(wide_reader));
        self.bind_reader_root(cx, ids!(phone_reader));
    }

    fn bind_reader_root(&mut self, cx: &mut Cx, root: &[LiveId]) {
        let root = self.view.widget(cx, root);
        let selected = self.ui.selected.and_then(|id| self.doc.as_ref().and_then(|d| d.message(id)).cloned());
        let show = selected.is_some();
        root.widget(cx, ids!(r_placeholder)).set_visible(cx, !show);
        root.widget(cx, ids!(r_subject)).set_visible(cx, show);
        root.widget(cx, ids!(sender)).set_visible(cx, show);
        root.widget(cx, ids!(r_body)).set_visible(cx, show);
        root.widget(cx, ids!(r_attachments)).set_visible(cx, show);
        let Some(message) = selected else {
            root.markdown(cx, ids!(r_body)).set_text(cx, "");
            return;
        };
        root.label(cx, ids!(r_subject)).set_text(cx, &message.subject_or_placeholder());
        root.label(cx, ids!(r_initials)).set_text(cx, &message.from.initials());
        root.label(cx, ids!(r_from)).set_text(cx, &message.from.display());
        root.label(cx, ids!(r_to)).set_text(
            cx,
            &format!(
                "To: {}",
                message.to.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ")
            ),
        );
        let cc = message.cc.iter().map(|a| a.display()).collect::<Vec<_>>().join(", ");
        root.widget(cx, ids!(r_cc)).set_visible(cx, !cc.is_empty());
        root.label(cx, ids!(r_cc)).set_text(cx, &format!("Cc: {cc}"));
        root.label(cx, ids!(r_date)).set_text(cx, &format_reader_date(message.date_secs));
        root.markdown(cx, ids!(r_body)).set_text(cx, &escape_markdown(&message.body_text));
        let chips = [ids!(chip0), ids!(chip1), ids!(chip2), ids!(chip3), ids!(chip4), ids!(chip5), ids!(chip6), ids!(chip7)];
        let labels = [ids!(a0), ids!(a1), ids!(a2), ids!(a3), ids!(a4), ids!(a5), ids!(a6), ids!(a7)];
        for i in 0..MAX_ATTACHMENTS {
            if let Some(att) = message.attachments.get(i) {
                root.widget(cx, chips[i]).set_visible(cx, true);
                root.label(cx, labels[i]).set_text(cx, &format!("{} · {}", att.name, att.size_label()));
            } else {
                root.widget(cx, chips[i]).set_visible(cx, false);
            }
        }
    }

    fn select_mailbox(&mut self, cx: &mut Cx, mailbox: Mailbox) {
        self.ui.list.mailbox = mailbox;
        self.ui.selected = None;
        self.keep_reader = false;
        self.rebuild_rows();
        self.ui.history = vec![Route::Mailboxes, Route::Messages];
        self.ui.list.scroll = ScrollAnchor::default();
        self.restore_scroll(cx);
        if self.compact { self.sync_stack(cx); }
        self.sync_chrome(cx);
        self.view.redraw(cx);
    }

    fn open_row(&mut self, cx: &mut Cx, id: MessageId) {
        self.capture_scroll(cx);
        let is_draft = self.doc.as_ref().and_then(|d| d.message(id)).map(|m| m.is_draft()).unwrap_or(false);
        if is_draft {
            self.open_compose(cx, Some(id), "Draft");
            return;
        }
        self.ui.selected = Some(id);
        self.ui.expanded.insert(id);
        let _ = self.mutate(cx, Command::SetRead { id, unread: false });
        if self.ui.list.unread_only && !self.rows.iter().any(|r| r.id == id) {
            self.keep_reader = true;
        }
        self.ui.history = vec![Route::Mailboxes, Route::Messages, Route::Read(id)];
        if self.compact { self.sync_stack(cx); }
        self.sync_chrome(cx);
    }

    fn pop_compact(&mut self, cx: &mut Cx) {
        match self.ui.history.last().copied() {
            Some(Route::Read(_)) => {
                self.ui.history.pop();
                self.ui.selected = None;
                self.keep_reader = false;
            }
            Some(Route::Messages) => { self.ui.history.pop(); }
            _ => {}
        }
        self.sync_stack(cx);
        self.restore_scroll(cx);
        self.sync_chrome(cx);
    }

    fn open_compose(&mut self, cx: &mut Cx, existing: Option<MessageId>, title: &str) {
        if self.load != LoadState::Ready {
            return;
        }
        let id = if let Some(id) = existing {
            id
        } else {
            match self.mutate(cx, Command::NewDraft) {
                Ok(m) => m.composer.unwrap(),
                Err(err) => {
                    self.status = err.to_string();
                    self.sync_status(cx);
                    return;
                }
            }
        };
        self.ui.composer = Some(id);
        self.bound_composer = None;
        self.view.label(cx, ids!(compose_title)).set_text(cx, title);
        self.bind_composer(cx, true);
        self.sync_chrome(cx);
        self.view.modal(cx, ids!(compose)).open(cx);
    }

    fn bind_composer(&mut self, cx: &mut Cx, force: bool) {
        let Some(id) = self.ui.composer else { return };
        if !force && self.bound_composer == Some(id) {
            return;
        }
        let Some(message) = self.doc.as_ref().and_then(|d| d.message(id)).cloned() else { return };
        let input = message.draft_input.clone().unwrap_or(DraftInput { to_raw: String::new(), cc_raw: String::new() });
        self.view.text_input(cx, ids!(compose_to)).set_text(cx, &input.to_raw);
        self.view.text_input(cx, ids!(compose_cc)).set_text(cx, &input.cc_raw);
        self.view.text_input(cx, ids!(compose_subject)).set_text(cx, &message.subject);
        self.view.text_input(cx, ids!(compose_body)).set_text(cx, &message.body_text);
        let chips = [ids!(fchip0), ids!(fchip1), ids!(fchip2), ids!(fchip3), ids!(fchip4), ids!(fchip5), ids!(fchip6), ids!(fchip7)];
        let labels = [ids!(f0), ids!(f1), ids!(f2), ids!(f3), ids!(f4), ids!(f5), ids!(f6), ids!(f7)];
        for i in 0..MAX_ATTACHMENTS {
            if let Some(att) = message.attachments.get(i) {
                self.view.widget(cx, chips[i]).set_visible(cx, true);
                self.view.label(cx, labels[i]).set_text(cx, &format!("{} · {}", att.name, att.size_label()));
            } else {
                self.view.widget(cx, chips[i]).set_visible(cx, false);
            }
        }
        self.bound_composer = Some(id);
    }

    fn composer_snapshot(&self, cx: &Cx) -> (String, String, String, String) {
        (
            self.view.text_input(cx, ids!(compose_to)).text(),
            self.view.text_input(cx, ids!(compose_cc)).text(),
            self.view.text_input(cx, ids!(compose_subject)).text(),
            self.view.text_input(cx, ids!(compose_body)).text(),
        )
    }

    fn save_composer_fields(&mut self, cx: &mut Cx) -> Result<(), MailError> {
        let Some(id) = self.ui.composer else { return Ok(()) };
        if matches!(self.pending_dismiss, Some(PendingDismiss::Send { .. })) { return Ok(()); }
        let (to_raw, cc_raw, subject, body) = self.composer_snapshot(cx);
        if self.doc.as_ref().and_then(|doc| doc.message(id)).is_some_and(|message| {
            message.subject == subject && message.body_text == body
                && message.draft_input.as_ref().is_some_and(|input| input.to_raw == to_raw && input.cc_raw == cc_raw)
        }) {
            self.editor_error = None;
            self.sync_status(cx);
            return Ok(());
        }
        let result = self.mutate(cx, Command::EditDraft { id, to_raw, cc_raw, subject, body });
        self.editor_error = result.as_ref().err().map(ToString::to_string);
        if result.is_err() { self.pending_dismiss = None; }
        self.sync_status(cx);
        result.map(|_| ())
    }

    fn close_compose(&mut self, cx: &mut Cx) {
        self.view.modal(cx, ids!(compose)).close(cx);
        self.ui.composer = None;
        self.bound_composer = None;
        self.pending_dismiss = None;
        self.editor_error = None;
        self.to_error.clear();
        self.cc_error.clear();
        self.view.widget(cx, ids!(compose_to_err)).set_visible(cx, false);
        self.view.widget(cx, ids!(compose_cc_err)).set_visible(cx, false);
    }

    fn cancel_compose(&mut self, cx: &mut Cx) {
        if self.pending_dismiss.is_some() { return; }
        if self.save_composer_fields(cx).is_err() { return; }
        let Some(id) = self.ui.composer else {
            self.close_compose(cx);
            return;
        };
        let empty = self
            .doc
            .as_ref()
            .and_then(|d| d.message(id))
            .map(|m| {
                let input = m.draft_input.as_ref();
                m.subject.trim().is_empty()
                    && m.body_text.trim().is_empty()
                    && input.map(|d| d.to_raw.trim().is_empty() && d.cc_raw.trim().is_empty()).unwrap_or(true)
            })
            .unwrap_or(true);
        if empty {
            if self.mutate(cx, Command::DiscardDraft { id }).is_ok() { self.close_compose(cx); }
        } else {
            self.open_sheet(cx, SheetKind::DraftCancel);
        }
    }

    fn send_composer(&mut self, cx: &mut Cx) {
        if self.pending_dismiss.is_some() { return; }
        if self.save_composer_fields(cx).is_err() { return; }
        let Some(id) = self.ui.composer else { return };
        let (to_raw, cc_raw, _, _) = self.composer_snapshot(cx);
        self.to_error.clear();
        self.cc_error.clear();
        if let Err(err) = parse_recipients(&to_raw) {
            if !to_raw.trim().is_empty() {
                self.to_error = err.to_string();
            }
        }
        if let Err(err) = parse_recipients(&cc_raw) {
            if !cc_raw.trim().is_empty() {
                self.cc_error = err.to_string();
            }
        }
        self.view.label(cx, ids!(compose_to_err)).set_text(cx, &self.to_error);
        self.view.widget(cx, ids!(compose_to_err)).set_visible(cx, !self.to_error.is_empty());
        self.view.label(cx, ids!(compose_cc_err)).set_text(cx, &self.cc_error);
        self.view.widget(cx, ids!(compose_cc_err)).set_visible(cx, !self.cc_error.is_empty());
        match self.mutate(cx, Command::SendDraft { id }) {
            Ok(_) => {
                self.pending_dismiss = Some(PendingDismiss::Send { revision: self.save.revision });
                self.sync_chrome(cx);
                if !self.save.pending() {
                    self.close_compose(cx);
                    self.status = "Saved to Sent · Demo".into();
                    self.sync_status(cx);
                }
            }
            Err(err) => {
                if self.to_error.is_empty() && to_raw.trim().is_empty() && cc_raw.trim().is_empty() {
                    self.to_error = err.to_string();
                    self.view.label(cx, ids!(compose_to_err)).set_text(cx, &self.to_error);
                    self.view.widget(cx, ids!(compose_to_err)).set_visible(cx, true);
                }
                self.status = err.to_string();
                self.sync_status(cx);
            }
        }
    }

    fn open_sheet(&mut self, cx: &mut Cx, kind: SheetKind) {
        self.sheet = kind;
        self.sheet_actions.clear();
        let mut labels = Vec::new();
        match kind {
            SheetKind::None => {}
            SheetKind::MessageActions => {
                self.view.label(cx, ids!(sheet_title)).set_text(cx, "Actions");
                self.sheet_actions.extend([
                    SheetChoice::Reply,
                    SheetChoice::Forward,
                    SheetChoice::Flag,
                    SheetChoice::Unflag,
                    SheetChoice::MarkRead,
                    SheetChoice::MarkUnread,
                    SheetChoice::MoveTo,
                ]);
                labels.extend(["Reply", "Forward", "Flag", "Unflag", "Mark Read", "Mark Unread", "Move…"]);
            }
            SheetKind::MoveTo => {
                self.view.label(cx, ids!(sheet_title)).set_text(cx, "Move to");
                for folder in [Folder::Inbox, Folder::Archive, Folder::Junk, Folder::Projects, Folder::Travel] {
                    self.sheet_actions.push(SheetChoice::MoveFolder(folder));
                    labels.push(folder.label());
                }
            }
            SheetKind::DraftCancel => {
                self.view.label(cx, ids!(sheet_title)).set_text(cx, "Draft");
                self.sheet_actions.extend([SheetChoice::SaveDraft, SheetChoice::DiscardDraft, SheetChoice::KeepEditing]);
                labels.extend(["Save Draft", "Discard Draft", "Keep Editing"]);
            }
            SheetKind::MailboxPicker => {
                self.view.label(cx, ids!(sheet_title)).set_text(cx, "Mailboxes");
                for mb in Mailbox::ALL {
                    self.sheet_actions.push(SheetChoice::Mailbox(mb));
                    labels.push(mb.label());
                }
            }
        }
        let buttons = [ids!(sheet0), ids!(sheet1), ids!(sheet2), ids!(sheet3), ids!(sheet4), ids!(sheet5), ids!(sheet6), ids!(sheet7), ids!(sheet8), ids!(sheet9)];
        for i in 0..10 {
            if let Some(text) = labels.get(i) {
                self.view.widget(cx, buttons[i]).set_visible(cx, true);
                self.view.button(cx, buttons[i]).set_text(cx, text);
            } else {
                self.view.widget(cx, buttons[i]).set_visible(cx, false);
            }
        }
        self.size_action_sheet(cx);
        self.view.modal(cx, ids!(action_sheet)).open(cx);
    }

    fn close_sheet(&mut self, cx: &mut Cx) {
        self.sheet = SheetKind::None;
        self.view.modal(cx, ids!(action_sheet)).close(cx);
    }

    fn handle_sheet_choice(&mut self, cx: &mut Cx, choice: SheetChoice) {
        self.close_sheet(cx);
        match choice {
            SheetChoice::Reply => {
                if let Some(id) = self.ui.selected {
                    if let Ok(m) = self.mutate(cx, Command::Reply { id }) {
                        if let Some(draft) = m.composer {
                            self.open_compose(cx, Some(draft), "Reply");
                        }
                    }
                }
            }
            SheetChoice::Forward => {
                if let Some(id) = self.ui.selected {
                    if let Ok(m) = self.mutate(cx, Command::Forward { id }) {
                        if let Some(draft) = m.composer {
                            self.open_compose(cx, Some(draft), "Forward");
                        }
                    }
                }
            }
            SheetChoice::Flag | SheetChoice::Unflag => {
                if let Some(id) = self.ui.selected {
                    let flagged = matches!(choice, SheetChoice::Flag);
                    let _ = self.mutate(cx, Command::SetFlagged { id, flagged });
                }
            }
            SheetChoice::MarkRead | SheetChoice::MarkUnread => {
                if let Some(id) = self.ui.selected {
                    let unread = matches!(choice, SheetChoice::MarkUnread);
                    let _ = self.mutate(cx, Command::SetRead { id, unread });
                }
            }
            SheetChoice::MoveTo => self.open_sheet(cx, SheetKind::MoveTo),
            SheetChoice::MoveFolder(folder) => {
                if let Some(id) = self.ui.selected {
                    let mutation = apply_or_status(self, cx, Command::Move { id, folder });
                    self.after_removal(cx, mutation);
                }
            }
            SheetChoice::SaveDraft => {
                if self.save_composer_fields(cx).is_err() { return; }
                self.pending_dismiss = Some(PendingDismiss::SaveDraft { revision: self.save.revision });
                if !self.save.pending() {
                    self.close_compose(cx);
                }
            }
            SheetChoice::DiscardDraft => {
                if let Some(id) = self.ui.composer {
                    if self.mutate(cx, Command::DiscardDraft { id }).is_err() { return; }
                }
                self.close_compose(cx);
            }
            SheetChoice::KeepEditing => {}
            SheetChoice::Mailbox(mailbox) => self.select_mailbox(cx, mailbox),
        }
    }

    fn after_removal(&mut self, cx: &mut Cx, mutation: Option<Mutation>) {
        let Some(mutation) = mutation else { return };
        if !mutation.left_results {
            return;
        }
        let Some(id) = mutation.selected else { return };
        let still = self.rows.iter().any(|r| r.id == id);
        if still {
            return;
        }
        self.ui.selected = None;
        self.keep_reader = false;
        self.ui.history = vec![Route::Mailboxes, Route::Messages];
        if self.compact { self.sync_stack(cx); }
        self.restore_scroll(cx);
        self.sync_chrome(cx);
    }

    fn editor_owns_focus(&self, cx: &Cx) -> bool {
        self.view.text_input(cx, ids!(compose_to)).key_focus(cx)
            || self.view.text_input(cx, ids!(compose_cc)).key_focus(cx)
            || self.view.text_input(cx, ids!(compose_subject)).key_focus(cx)
            || self.view.text_input(cx, ids!(compose_body)).key_focus(cx)
            || self.view.text_input(cx, ids!(wide_search)).key_focus(cx)
            || self.view.text_input(cx, ids!(phone_search)).key_focus(cx)
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        self.capture_scroll(cx);
        if self.view.button(cx, ids!(save_retry)).clicked(actions)
            || self.view.button(cx, ids!(compose_retry)).clicked(actions) {
            self.flush_save(cx);
        }
        if self.view.button(cx, ids!(retry)).clicked(actions) {
            if let Some(storage) = self.storage.as_ref() {
                self.load = LoadState::Loading;
                self.load_id = Some(storage.get(cx, STORAGE_KEY));
                self.sync_chrome(cx);
            } else {
                self.fail_load(cx, "Mail storage is unavailable".into());
            }
        }
        if let Some(mailbox) = mailbox_click(self, cx, actions) {
            self.select_mailbox(cx, mailbox);
        }
        if self.glass(cx, ids!(wide_compose), actions)
            || self.glass(cx, ids!(phone_compose_mb), actions)
            || self.glass(cx, ids!(phone_compose_list), actions)
            || self.glass(cx, ids!(phone_compose_read), actions)
        {
            self.open_compose(cx, None, "New Message");
        }
        if self.glass(cx, ids!(wide_reply), actions) {
            self.handle_sheet_choice(cx, SheetChoice::Reply);
        }
        if self.glass(cx, ids!(wide_forward), actions) {
            self.handle_sheet_choice(cx, SheetChoice::Forward);
        }
        if self.glass(cx, ids!(wide_archive), actions) || self.glass(cx, ids!(phone_archive), actions) {
            if let Some(id) = self.ui.selected {
                let mutation = apply_or_status(self, cx, Command::Archive { id });
                self.after_removal(cx, mutation);
            }
        }
        if self.glass(cx, ids!(wide_delete), actions) || self.glass(cx, ids!(phone_delete), actions) {
            if let Some(id) = self.ui.selected {
                let mutation = apply_or_status(self, cx, Command::Delete { id });
                self.after_removal(cx, mutation);
            }
        }
        if self.glass(cx, ids!(wide_flag), actions) {
            if let Some(id) = self.ui.selected {
                let flagged = self.doc.as_ref().and_then(|d| d.message(id)).map(|m| !m.flagged).unwrap_or(true);
                let _ = self.mutate(cx, Command::SetFlagged { id, flagged });
            }
        }
        if self.glass(cx, ids!(wide_more), actions) || self.glass(cx, ids!(phone_reply), actions) {
            self.open_sheet(cx, SheetKind::MessageActions);
        }
        if self.glass(cx, ids!(phone_move), actions) {
            self.open_sheet(cx, SheetKind::MoveTo);
        }
        if self.view.button(cx, ids!(wide_mailbox_btn)).clicked(actions) {
            self.open_sheet(cx, SheetKind::MailboxPicker);
        }
        if self.glass(cx, ids!(list_back), actions) || self.glass(cx, ids!(read_back), actions) {
            self.pop_compact(cx);
        }
        if self.view.button(cx, ids!(phone_unread)).clicked(actions) {
            self.ui.list.unread_only = !self.ui.list.unread_only;
            self.rebuild_rows();
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        if self.view.button(cx, ids!(clear_search)).clicked(actions) || self.view.button(cx, ids!(phone_clear_search)).clicked(actions) {
            self.ui.list.query.clear();
            self.view.text_input(cx, ids!(wide_search)).set_text(cx, "");
            self.view.text_input(cx, ids!(phone_search)).set_text(cx, "");
            self.rebuild_rows();
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        if let Some(text) = self.view.text_input(cx, ids!(wide_search)).changed(actions) {
            self.ui.list.query = text;
            self.rebuild_rows();
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        if let Some(text) = self.view.text_input(cx, ids!(phone_search)).changed(actions) {
            self.ui.list.query = text;
            self.rebuild_rows();
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        if self.view.glass_segmented(cx, ids!(wide_scope)).changed(actions) {
            if let Some(seg) = self.view.glass_segmented(cx, ids!(wide_scope)).borrow() {
                self.ui.list.scope = if seg.selected == 0 { SearchScope::CurrentMailbox } else { SearchScope::AllMail };
                self.rebuild_rows();
            }
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        if self.view.glass_segmented(cx, ids!(phone_scope)).changed(actions) {
            if let Some(seg) = self.view.glass_segmented(cx, ids!(phone_scope)).borrow() {
                self.ui.list.scope = if seg.selected == 0 { SearchScope::CurrentMailbox } else { SearchScope::AllMail };
                self.rebuild_rows();
            }
            self.restore_scroll(cx);
            self.sync_chrome(cx);
        }
        self.handle_list_clicks(cx, ids!(wide_list), actions);
        self.handle_list_clicks(cx, ids!(phone_list), actions);
        if self.view.button(cx, ids!(compose_cancel)).clicked(actions) {
            self.cancel_compose(cx);
        }
        if self.view.button(cx, ids!(compose_send)).clicked(actions) {
            self.send_composer(cx);
        }
        if self.view.text_input(cx, ids!(compose_to)).changed(actions).is_some()
            || self.view.text_input(cx, ids!(compose_cc)).changed(actions).is_some()
            || self.view.text_input(cx, ids!(compose_subject)).changed(actions).is_some()
            || self.view.text_input(cx, ids!(compose_body)).changed(actions).is_some()
        {
            let _ = self.save_composer_fields(cx);
        }
        if let Some((_, mods)) = self.view.text_input(cx, ids!(compose_body)).returned(actions) {
            if mods.is_primary() || mods.control {
                self.send_composer(cx);
            }
        }
        if self.view.text_input(cx, ids!(compose_to)).escaped(actions)
            || self.view.text_input(cx, ids!(compose_cc)).escaped(actions)
            || self.view.text_input(cx, ids!(compose_subject)).escaped(actions)
            || self.view.text_input(cx, ids!(compose_body)).escaped(actions)
        {
            self.cancel_compose(cx);
        }
        let sheet_btns = [ids!(sheet0), ids!(sheet1), ids!(sheet2), ids!(sheet3), ids!(sheet4), ids!(sheet5), ids!(sheet6), ids!(sheet7), ids!(sheet8), ids!(sheet9)];
        for (i, id) in sheet_btns.iter().enumerate() {
            if self.view.button(cx, *id).clicked(actions) {
                if let Some(choice) = self.sheet_actions.get(i).copied() {
                    self.handle_sheet_choice(cx, choice);
                    break;
                }
            }
        }
        if self.view.button(cx, ids!(sheet_cancel)).clicked(actions) || self.view.modal(cx, ids!(action_sheet)).dismissed(actions) {
            self.close_sheet(cx);
        }
    }

    fn handle_list_clicks(&mut self, cx: &mut Cx, list_id: &[LiveId], actions: &Actions) {
        let list = self.view.portal_list(cx, list_id);
        let mut picked = None;
        for (index, item) in list.items_with_actions(actions) {
            if item.button(cx, ids!(hit)).clicked(actions) {
                picked = Some(index);
                break;
            }
        }
        if let Some(index) = picked {
            if let Some(row) = self.rows.get(index) {
                self.open_row(cx, row.id);
            }
        }
    }

    fn glass(&self, cx: &Cx, id: &[LiveId], actions: &Actions) -> bool {
        self.view.glass_button(cx, id).clicked(actions)
    }

    fn draw_messages(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        list.set_item_range(cx, 0, self.rows.len());
        while let Some(index) = list.next_visible_item(cx) {
            let Some(row) = self.rows.get(index).cloned() else { continue };
            let item = list.item(cx, index, live_id!(Message));
            self.bind_message_row(cx, item.clone(), row);
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    fn bind_message_row(&self, cx: &mut Cx, mut item: WidgetRef, row: MessageRow) {
        let today = self
            .doc
            .as_ref()
            .map(|d| d.seed_anchor.midnight_utc)
            .unwrap_or(0);
        let metrics = self.last_metrics.unwrap_or_else(|| layout_for(1240.0, 800.0));
        let compact = metrics.kind == LayoutKind::Compact && !metrics.short;
        let sender_size = if compact { 12.75 } else { 9.75 };
        let date_w = if compact { 76.0 } else { 68.0 };
        let preview_lines = if metrics.short { 1 } else { 2 };
        let preview_h = if metrics.short { 16.0 } else if compact { 40.0 } else { 32.0 };
        script_apply_eval!(cx, item, { height: #(metrics.row_h) });
        let left = if compact {32.0} else {28.0};
        let top = if compact {10.0} else {9.0};
        if let Some(mut content) = item.widget(cx, ids!(content)).borrow_mut::<View>() {
            content.layout.padding = Inset {left, right: 16.0, top, bottom: if compact {12.0} else {9.0}};
        }
        for (id, height) in [(ids!(sender_line), if compact {22.0} else {17.0}), (ids!(subject_line), if compact {20.0} else {17.0})] {
            if let Some(mut line) = item.widget(cx, id).borrow_mut::<View>() { line.walk.height = Size::Fixed(height); }
        }
        if let Some(mut dot) = item.widget(cx, ids!(dot)).borrow_mut::<View>() {
            let size = if compact {8.0} else {6.0};
            dot.walk.width = Size::Fixed(size); dot.walk.height = Size::Fixed(size);
            dot.walk.margin = Inset {left: if compact {14.0} else {12.0}, top: top + 7.0, ..Default::default()};
        }
        if let Some(mut separator) = item.widget(cx, ids!(separator)).borrow_mut::<View>() {
            separator.walk.margin = Inset {left, top: metrics.row_h - 0.5, ..Default::default()};
        }
        let mut date = item.widget(cx, ids!(date));
        script_apply_eval!(cx, date, { width: #(date_w) });
        let mut preview_w = item.widget(cx, ids!(preview));
        script_apply_eval!(cx, preview_w, { height: #(preview_h) max_lines: #(preview_lines) });
        if let Some(mut label) = item.label(cx, ids!(sender)).borrow_mut() {
            label.draw_text.text_style.line_spacing = 1.0;
            label.max_lines = 1;
            label.walk.width = Size::fill();
            label.draw_text.text_style.font_size = sender_size;
        }
        if let Some(mut label) = item.label(cx, ids!(subject)).borrow_mut() {
            label.draw_text.text_style.line_spacing = 1.0;
            label.max_lines = 1;
            label.walk.width = Size::fill();
            label.draw_text.text_style.font_size = if compact { 11.25 } else { 9.75 };
        }
        if let Some(mut label) = item.label(cx, ids!(preview)).borrow_mut() {
            label.draw_text.text_style.line_spacing = 1.0;
            label.max_lines = preview_lines;
            label.walk.width = Size::fill();
            label.draw_text.text_style.font_size = if compact { 11.25 } else { 9.0 };
        }
        if let Some(mut label) = item.label(cx, ids!(date)).borrow_mut() {
            label.draw_text.text_style.line_spacing = 1.0;
            label.max_lines = 1;
            label.walk.width = Size::Fixed(date_w);
            label.draw_text.text_style.font_size = if compact { 9.75 } else { 8.25 };
        }
        item.widget(cx, ids!(sel)).set_visible(cx, self.ui.selected == Some(row.id));
        item.widget(cx, ids!(dot)).set_visible(cx, row.unread);
        item.widget(cx, ids!(flag)).set_visible(cx, row.flagged);
        item.widget(cx, ids!(clip)).set_visible(cx, row.has_attachment);
        let sender = if row.thread_count > 1 {
            format!("{} ({})", row.sender, row.thread_count)
        } else {
            row.sender.clone()
        };
        item.label(cx, ids!(sender)).set_text(cx, &sender);
        item.label(cx, ids!(subject)).set_text(cx, &row.subject);
        let preview = if !self.ui.list.query.trim().is_empty() && self.ui.list.scope == SearchScope::AllMail {
            format!("{} · {}", row.mailbox.label(), row.preview)
        } else {
            row.preview.clone()
        };
        item.label(cx, ids!(preview)).set_text(cx, &preview);
        item.label(cx, ids!(date)).set_text(cx, &format_list_date(row.date_secs, today));
    }

}

fn apply_or_status(view: &mut MailView, cx: &mut Cx, command: Command) -> Option<Mutation> {
    match view.mutate(cx, command) {
        Ok(m) => Some(m),
        Err(err) => {
            view.status = err.to_string();
            view.sync_status(cx);
            None
        }
    }
}

fn side_id(mailbox: Mailbox) -> Option<&'static [LiveId]> {
    Some(match mailbox {
        Mailbox::Folder(Folder::Inbox) => ids!(side_inbox),
        Mailbox::Vips => ids!(side_vips),
        Mailbox::Flagged => ids!(side_flagged),
        Mailbox::Folder(Folder::Drafts) => ids!(side_drafts),
        Mailbox::Folder(Folder::Sent) => ids!(side_sent),
        Mailbox::Folder(Folder::Archive) => ids!(side_archive),
        Mailbox::Folder(Folder::Junk) => ids!(side_junk),
        Mailbox::Folder(Folder::Trash) => ids!(side_trash),
        Mailbox::Folder(Folder::Projects) => ids!(side_projects),
        Mailbox::Folder(Folder::Travel) => ids!(side_travel),
    })
}

fn mb_id(mailbox: Mailbox) -> Option<&'static [LiveId]> {
    Some(match mailbox {
        Mailbox::Folder(Folder::Inbox) => ids!(mb_inbox),
        Mailbox::Vips => ids!(mb_vips),
        Mailbox::Flagged => ids!(mb_flagged),
        Mailbox::Folder(Folder::Drafts) => ids!(mb_drafts),
        Mailbox::Folder(Folder::Sent) => ids!(mb_sent),
        Mailbox::Folder(Folder::Archive) => ids!(mb_archive),
        Mailbox::Folder(Folder::Junk) => ids!(mb_junk),
        Mailbox::Folder(Folder::Trash) => ids!(mb_trash),
        Mailbox::Folder(Folder::Projects) => ids!(mb_projects),
        Mailbox::Folder(Folder::Travel) => ids!(mb_travel),
    })
}

fn mailbox_click(view: &MailView, cx: &Cx, actions: &Actions) -> Option<Mailbox> {
    for mb in Mailbox::ALL {
        if let Some(id) = side_id(mb) {
            if view.view.button(cx, id).clicked(actions) {
                return Some(mb);
            }
        }
        if let Some(id) = mb_id(mb) {
            if view.view.button(cx, id).clicked(actions) {
                return Some(mb);
            }
        }
    }
    None
}

impl Widget for MailView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        // TextInput's read-only mode guards typing/IME/undo, but its native
        // TextCut handler still edits. Keep the submitted composer immutable
        // until persistence finishes, while pointer input can reach Retry.
        if matches!(self.pending_dismiss, Some(PendingDismiss::Send { .. }))
            && matches!(event, Event::TextCut(_)) {
            return;
        }
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape {
                if self.sheet != SheetKind::None {
                    self.close_sheet(cx);
                    return;
                }
                if self.ui.composer.is_some() {
                    self.cancel_compose(cx);
                    return;
                }
            }
            if ke.modifiers.is_primary() && ke.key_code == KeyCode::KeyN && !self.editor_owns_focus(cx) {
                self.open_compose(cx, None, "New Message");
                return;
            }
        }
        if let Event::BackPressed { .. } = event {
            if event.back_pressed() {
                if self.sheet != SheetKind::None {
                    self.close_sheet(cx);
                    return;
                }
                if self.ui.composer.is_some() {
                    self.cancel_compose(cx);
                    return;
                }
                if self.compact {
                    self.pop_compact(cx);
                    return;
                }
            }
        }
        self.view.handle_event(cx, event, scope);
        if self.compact { self.sync_stack(cx); }
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        self.update_layout(cx, size);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_messages(cx, &mut list);
            }
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::storage::{StorageError, StorageOp};
    use makepad_widgets::makepad_draw::text::selection::Cursor;

    fn root() -> (Cx, WidgetRef) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        let root = cx.with_vm(|vm| {
            makepad_widgets::script_mod(vm);
            makepad_wm_theme::apply(vm);
            crate::script_mod(vm);
            let value = script_eval!(vm, {use mod.widgets.* MailView{}});
            let root = WidgetRef::script_from_value(vm, value);
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:?}");
            root
        });
        {
            let mut view = root.borrow_mut::<MailView>().unwrap();
            view.started = true;
            view.set_storage(cx.storage("mail.fix.unit"));
            view.install_doc(&mut cx, seed::generate(seed::test_anchor()), false);
            // Mint the cached phone templates so control/geometry tests cover both presentations.
            let nav = view.view.stack_navigation(&cx, ids!(phone));
            nav.view_by_id(&mut cx, live_id!(messages));
            nav.view_by_id(&mut cx, live_id!(message));
            view.update_layout(&mut cx, dvec2(1240.0, 800.0));
        }
        (cx, root)
    }

    fn response(id: u64, result: Result<StorageResult, StorageError>) -> StorageResponse {
        StorageResponse {request_id: StorageRequestId(id), namespace: "mail.fix.unit".into(), op: StorageOp::Set, result}
    }

    fn click(view: &mut MailView, cx: &mut Cx, id: &[LiveId]) {
        let button = view.view.button(cx, id);
        let actions = cx.capture_actions(|cx| cx.widget_action(button.widget_uid(), ButtonAction::Clicked(KeyModifiers::default())));
        view.handle_actions(cx, &actions);
    }

    fn no_script_errors(cx: &mut Cx) {
        cx.with_vm(|vm| {
            let errors = vm.take_errors();
            assert!(errors.is_empty(), "{errors:?}");
        });
    }

    // Record real layout and widget areas without a window or GPU. Keep these
    // resources alive while dispatching the resulting input/actions.
    fn draw(cx: &mut Cx, view: &mut MailView, size: DVec2) -> (DrawPass, DrawList2d, Overlay) {
        use makepad_widgets::makepad_draw::cx_draw::CxDraw;
        let pass = DrawPass::new(cx);
        pass.set_size(cx, size);
        let mut list = DrawList2d::new(cx);
        let overlay = Overlay {draw_list: DrawList::new(cx)};
        cx.redraw_all();
        let event = std::mem::take(&mut cx.new_draw_event);
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, Some(1.0));
        list.begin_always(&mut cx2d);
        overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_overlay());
        assert!(view.draw_walk(&mut cx2d, &mut Scope::empty(), Walk::fill()).is_ok());
        cx2d.end_pass_sized_turtle();
        overlay.end(&mut cx2d);
        list.end(&mut cx2d);
        cx2d.end_pass(&pass);
        (pass, list, overlay)
    }

    fn dispatch(view: &mut MailView, cx: &mut Cx, event: Event) {
        // Advance the platform's queued focus request before injecting input.
        cx.action(());
        cx.handle_actions();
        let actions = cx.capture_actions(|cx| view.handle_event(cx, &event, &mut Scope::empty()));
        view.handle_event(cx, &Event::Actions(actions), &mut Scope::empty());
    }

    #[test]
    fn toolbar_targets_and_search_fit_at_700_by_800_and_across_wide_sizes() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        for (width, height) in [(700.0, 800.0), (771.0, 800.0), (772.0, 800.0), (874.0, 300.0), (1100.0, 800.0), (1240.0, 800.0)] {
            let _frame = draw(&mut cx, &mut view, dvec2(width, height));
            let mut right = 0.0;
            for id in [ids!(wide_title), ids!(wide_mailbox_btn), ids!(wide_compose), ids!(wide_reply), ids!(wide_forward), ids!(wide_archive), ids!(wide_delete), ids!(wide_flag), ids!(wide_more), ids!(wide_search)] {
                let widget = view.view.widget(&cx, id);
                if !widget.visible() { continue; }
                let rect = widget.area().rect(&cx);
                assert!(rect.size.x > 0.0, "{id:?}: {rect:?}");
                assert!(rect.pos.x >= right && rect.pos.x + rect.size.x <= width,
                    "{width}x{height} {id:?}: {rect:?}, previous right {right}");
                assert!(rect.pos.y >= 0.0 && rect.pos.y + rect.size.y <= view.last_metrics.unwrap().toolbar_h,
                    "{width}x{height} {id:?}: {rect:?}");
                right = rect.pos.x + rect.size.x;
            }
            for id in [ids!(wide_compose), ids!(wide_reply), ids!(wide_archive), ids!(wide_delete), ids!(wide_more)] {
                let rect = view.view.widget(&cx, id).area().rect(&cx);
                assert_eq!(rect.size, dvec2(44.0, 44.0));
            }
            if width == 700.0 {
                assert!(!view.view.widget(&cx, ids!(wide_forward)).visible());
                assert!(!view.view.widget(&cx, ids!(wide_flag)).visible());
                view.open_sheet(&mut cx, SheetKind::MessageActions);
                assert!(view.sheet_actions.contains(&SheetChoice::Forward));
                assert!(view.sheet_actions.contains(&SheetChoice::Flag));
                view.close_sheet(&mut cx);
            }
        }
        no_script_errors(&mut cx);
    }

    #[test]
    fn pending_send_rejects_late_typing_paste_cut_and_ime_until_ack_then_unlocks() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.open_compose(&mut cx, Some(MessageId(39)), "Draft");
        let _frame = draw(&mut cx, &mut view, dvec2(1240.0, 800.0));
        let body = view.view.text_input(&cx, ids!(compose_body));
        body.take_key_focus(&mut cx);
        let before = body.text();
        dispatch(&mut view, &mut cx, Event::TextInput(TextInputEvent { input: "before".into(), ..Default::default() }));
        assert_ne!(body.text(), before, "the event path must accept edits before Send");
        view.send_composer(&mut cx);
        let submitted = view.composer_snapshot(&cx);
        let doc = view.doc.clone();
        for id in [ids!(compose_to), ids!(compose_cc), ids!(compose_subject), ids!(compose_body)] {
            let input = view.view.text_input(&cx, id);
            input.take_key_focus(&mut cx); // A click can refocus the field after Send.
            for event in [
                Event::TextInput(TextInputEvent {input: "late".into(), ..Default::default()}),
                Event::TextInput(TextInputEvent {input: "paste".into(), was_paste: true, ..Default::default()}),
                Event::TextRangeReplace(TextRangeReplaceEvent {start: 0, end: 1, text: "ime".into(), replaced_text: None, fallback_to_insert: true}),
                Event::KeyDown(KeyEvent {key_code: KeyCode::Backspace, ..Default::default()}),
            ] {
                dispatch(&mut view, &mut cx, event);
                assert_eq!(view.composer_snapshot(&cx), submitted);
            }
            input.set_cursor(&mut cx, Cursor {index: 0, prefer_next_row: false}, false);
            input.set_cursor(&mut cx, Cursor {index: input.text().len(), prefer_next_row: false}, true);
            dispatch(&mut view, &mut cx, Event::TextCut(TextClipboardEvent {response: Default::default()}));
            assert_eq!(view.composer_snapshot(&cx), submitted);
        }
        assert_eq!(view.doc, doc);
        // The earlier edit's acknowledgement must not unlock the pending Send.
        let first = view.save.in_flight.unwrap();
        view.on_storage(&mut cx, &[response(first.0, Ok(StorageResult::Unit))]);
        let send = view.save.in_flight.unwrap();
        view.on_storage(&mut cx, &[response(send.0, Err(StorageError::Io("disk full".into())))]);
        body.take_key_focus(&mut cx);
        dispatch(&mut view, &mut cx, Event::TextInput(TextInputEvent {input: "late".into(), ..Default::default()}));
        assert_eq!(view.composer_snapshot(&cx), submitted);
        click(&mut view, &mut cx, ids!(compose_retry));
        let retry = view.save.in_flight.unwrap();
        view.on_storage(&mut cx, &[response(retry.0, Ok(StorageResult::Unit))]);
        assert!(view.ui.composer.is_none());
        assert_eq!(view.doc, doc);
        view.open_compose(&mut cx, Some(MessageId(40)), "Draft");
        body.take_key_focus(&mut cx);
        let before = body.text();
        dispatch(&mut view, &mut cx, Event::TextInput(TextInputEvent {input: "editable".into(), ..Default::default()}));
        assert_ne!(body.text(), before);
        assert_eq!(view.doc.as_ref().unwrap().message(MessageId(40)).unwrap().body_text, body.text());
        no_script_errors(&mut cx);
    }

    #[test]
    fn search_at_first_row_settles_without_redraw_or_repeated_edge_actions() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        let search = view.view.text_input(&cx, ids!(wide_search));
        let actions = cx.capture_actions(|cx| cx.widget_action(search.widget_uid(), TextInputAction::Changed("harbor".into())));
        view.handle_actions(&mut cx, &actions);
        assert!(!view.rows.is_empty());
        assert_eq!(view.view.portal_list(&cx, ids!(wide_list)).borrow().unwrap().first_id(), 0);
        let mut frame = None;
        for iteration in 0..3 {
            let actions = cx.capture_actions(|cx| frame = Some(draw(cx, &mut view, dvec2(1240.0, 800.0))));
            let list = view.view.portal_list(&cx, ids!(wide_list));
            let reached_start = actions.iter().filter_map(|action| action.as_widget_action())
                .filter(|action| action.widget_uid == list.widget_uid())
                .any(|action| matches!(action.cast(), PortalListAction::ReachedStart));
            assert_eq!(reached_start, iteration == 0, "iteration {iteration}");
            cx.new_draw_event = DrawEvent::default();
            view.handle_event(&mut cx, &Event::Actions(actions), &mut Scope::empty());
            assert!(!cx.new_draw_event.will_redraw(), "idle actions requested another frame");
            view.restore_scroll(&cx); // Restoring an unchanged anchor must preserve edge state.
        }
        no_script_errors(&mut cx);
    }

    #[test]
    fn invalid_editor_contents_abort_send_save_cancel_and_polite_close() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.open_compose(&mut cx, Some(MessageId(39)), "Draft");
        let before = view.doc.clone();
        for (field, text) in [(ids!(compose_subject), "é".repeat(257)), (ids!(compose_body), "x".repeat(MAX_BODY_BYTES + 1))] {
            view.bind_composer(&mut cx, true);
            view.view.text_input(&cx, field).set_text(&mut cx, &text);
            view.send_composer(&mut cx);
            assert_eq!(view.doc, before);
            assert_eq!(view.ui.composer, Some(MessageId(39)));
            assert!(view.pending_dismiss.is_none());
            assert!(view.editor_error.is_some());
            view.handle_sheet_choice(&mut cx, SheetChoice::SaveDraft);
            assert!(view.pending_dismiss.is_none());
            view.cancel_compose(&mut cx);
            assert_eq!(view.ui.composer, Some(MessageId(39)));
            assert!(!view.request_close(&mut cx));
            assert!(!view.should_quit());
            assert_eq!(view.doc, before);
            assert_eq!(view.view.text_input(&cx, field).text(), text);
        }
        no_script_errors(&mut cx);
    }

    #[test]
    fn loading_success_error_and_retry_update_shell_without_resize_or_reseed() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        let bytes = storage::encode_document(view.doc.as_ref().unwrap()).unwrap();
        view.doc = None; view.load = LoadState::Loading; view.load_id = Some(StorageRequestId(900));
        view.sync_chrome(&mut cx);
        assert!(view.view.widget(&cx, ids!(loading)).visible());
        view.on_storage(&mut cx, &[response(900, Ok(StorageResult::Value(Some(bytes))))]);
        assert!(!view.view.widget(&cx, ids!(loading)).visible());
        assert!(view.view.widget(&cx, ids!(wide)).visible());
        view.load_id = Some(StorageRequestId(901)); view.doc = None;
        view.on_storage(&mut cx, &[response(901, Ok(StorageResult::Value(Some(Vec::new()))))]);
        assert!(view.view.widget(&cx, ids!(error_page)).visible());
        assert!(view.doc.is_none());
        click(&mut view, &mut cx, ids!(retry));
        assert!(view.view.widget(&cx, ids!(loading)).visible());
        assert!(!view.view.widget(&cx, ids!(error_page)).visible());
        let id = view.load_id.unwrap().0;
        view.on_storage(&mut cx, &[response(id, Err(StorageError::Io("unreadable".into())))]);
        assert!(view.doc.is_none());
        assert!(view.view.widget(&cx, ids!(error_page)).visible());
        no_script_errors(&mut cx);
    }

    #[test]
    fn failed_save_retains_close_and_editor_until_actual_retry_is_acknowledged() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.open_compose(&mut cx, Some(MessageId(39)), "Draft");
        view.view.text_input(&cx, ids!(compose_body)).set_text(&mut cx, "revision one");
        view.save_composer_fields(&mut cx).unwrap();
        let r1 = view.save.in_flight.unwrap();
        view.view.text_input(&cx, ids!(compose_body)).set_text(&mut cx, "revision three");
        view.handle_sheet_choice(&mut cx, SheetChoice::SaveDraft);
        assert!(!view.request_close(&mut cx));
        view.on_storage(&mut cx, &[response(r1.0, Err(StorageError::Io("disk full".into())))]);
        assert!(!view.should_quit());
        assert!(view.ui.composer.is_some());
        assert!(view.save.pending());
        assert!(view.view.widget(&cx, ids!(compose_retry)).visible());
        let load_id = view.load_id;
        click(&mut view, &mut cx, ids!(compose_retry));
        assert_eq!(view.load_id, load_id, "retry saves; it never reloads");
        let retry = view.save.in_flight.unwrap();
        assert_eq!(retry.1, view.save.revision);
        view.on_storage(&mut cx, &[response(r1.0, Ok(StorageResult::Unit))]);
        assert!(view.ui.composer.is_some(), "a stale ack cannot dismiss");
        assert!(!view.should_quit());
        view.on_storage(&mut cx, &[response(retry.0, Ok(StorageResult::Unit))]);
        assert!(!view.save.pending());
        assert!(view.ui.composer.is_none());
        assert!(view.should_quit());
        assert_eq!(view.doc.as_ref().unwrap().message(MessageId(39)).unwrap().body_text, "revision three");
        view.closing = false; view.quit_ready = false;
        view.open_compose(&mut cx, Some(MessageId(39)), "Draft");
        view.send_composer(&mut cx);
        let send = view.save.in_flight.unwrap();
        assert_eq!(view.doc.as_ref().unwrap().message(MessageId(39)).unwrap().folder, Folder::Sent);
        assert!(view.ui.composer.is_some(), "Send waits for its storage acknowledgement");
        let revision = view.save.revision;
        view.send_composer(&mut cx);
        assert_eq!(view.save.revision, revision, "a pending send cannot duplicate the message");
        view.on_storage(&mut cx, &[response(send.0, Err(StorageError::Io("write failed".into())))]);
        assert!(view.ui.composer.is_some());
        click(&mut view, &mut cx, ids!(compose_retry));
        let retry = view.save.in_flight.unwrap();
        view.on_storage(&mut cx, &[response(retry.0, Ok(StorageResult::Unit))]);
        assert!(view.ui.composer.is_none());
        assert_eq!(view.status, "Saved to Sent · Demo");
        no_script_errors(&mut cx);
    }

    #[test]
    fn resize_applies_short_chrome_and_preserves_query_scroll_draft_and_caret() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.ui.list.query = "harbor".into(); view.ui.list.scope = SearchScope::AllMail;
        view.rebuild_rows(); view.sync_chrome(&mut cx);
        view.view.portal_list(&cx, ids!(wide_list)).set_first_id_and_scroll(3, -17.0);
        let anchor_id = view.rows[3].id;
        view.open_compose(&mut cx, Some(MessageId(39)), "Draft");
        let input = view.view.text_input(&cx, ids!(compose_body));
        input.set_text(&mut cx, "hello café\nsecond line");
        input.set_cursor(&mut cx, Cursor {index: 6, prefer_next_row: false}, false);
        view.save_composer_fields(&mut cx).unwrap();
        let before = view.doc.clone();
        for (width, height) in [(600.0, 780.0), (402.0, 780.0), (874.0, 300.0), (1240.0, 800.0)] {
            view.update_layout(&mut cx, dvec2(width, height));
            assert_eq!(input.text(), "hello café\nsecond line");
            assert_eq!(input.cursor().index, 6);
            assert_eq!(view.doc, before);
            assert_eq!(view.ui.list.scroll.first_message, Some(anchor_id));
            assert_eq!(view.ui.list.scroll.offset_points, -17.0);
            for id in [ids!(wide_search), ids!(phone_search)] {
                assert_eq!(view.view.text_input(&cx, id).text(), "harbor");
            }
            for id in [ids!(wide_scope), ids!(phone_scope)] {
                assert_eq!(view.view.glass_segmented(&cx, id).borrow().unwrap().selected, 1);
            }
            let sheet = view.view.widget(&cx, ids!(compose_sheet));
            assert_eq!(sheet.walk(&mut cx).width, Size::Fixed(width.min(680.0)));
            if height == 300.0 {
                assert_eq!(sheet.walk(&mut cx).height, Size::Fixed(284.0));
                assert_eq!(view.view.widget(&cx, ids!(wide_toolbar)).walk(&mut cx).height, Size::Fixed(48.0));
                assert_eq!(view.view.widget(&cx, ids!(heading)).walk(&mut cx).height, Size::Fixed(44.0));
                assert!(!view.view.widget(&cx, ids!(wide_title)).visible());
                assert!(!view.view.widget(&cx, ids!(wide_forward)).visible());
                assert!(!view.view.widget(&cx, ids!(wide_flag)).visible());
                assert!(!view.view.widget(&cx, ids!(wide_status_bar)).visible());
            }
        }
        no_script_errors(&mut cx);
    }

    fn finish_push(view: &mut MailView, cx: &mut Cx, id: LiveId) {
        let nav = view.view.stack_navigation(cx, ids!(phone));
        let child = nav.view_by_id(cx, id);
        let actions = cx.capture_actions(|cx| cx.widget_action(child.widget_uid(), StackNavigationTransitionAction::ShowDone));
        nav.borrow_mut().unwrap().handle_actions(cx, &actions, &mut Scope::empty());
        assert!(!nav.is_transitioning());
    }

    #[test]
    fn reader_resize_and_inflight_navigation_reconcile_to_root_history() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.open_row(&mut cx, MessageId(1));
        assert_eq!(view.ui.history.last(), Some(&Route::Read(MessageId(1))));
        view.update_layout(&mut cx, dvec2(402.0, 780.0));
        let nav = view.view.stack_navigation(&cx, ids!(phone));
        assert_eq!(nav.destination_view(), Some(live_id!(messages)));
        finish_push(&mut view, &mut cx, live_id!(messages));
        view.sync_stack(&mut cx);
        assert_eq!(nav.destination_view(), Some(live_id!(message)));
        finish_push(&mut view, &mut cx, live_id!(message));
        view.update_layout(&mut cx, dvec2(1240.0, 800.0));
        view.update_layout(&mut cx, dvec2(402.0, 780.0));
        assert!(!nav.is_transitioning());
        assert_eq!(nav.current_view(), Some(live_id!(message)));
        view.pop_compact(&mut cx);
        assert_eq!(nav.destination_view(), Some(live_id!(messages)));
        assert_eq!(view.ui.history, vec![Route::Mailboxes, Route::Messages]);
        // A second intent during the first transition is retained, not discarded.
        view.pop_compact(&mut cx);
        assert_eq!(view.ui.history, vec![Route::Mailboxes]);
        let child = nav.view_by_id(&mut cx, live_id!(message));
        let actions = cx.capture_actions(|cx| cx.widget_action(child.widget_uid(), StackNavigationTransitionAction::HideEnd(nav.widget_uid())));
        nav.borrow_mut().unwrap().handle_actions(&mut cx, &actions, &mut Scope::empty());
        view.sync_stack(&mut cx);
        assert_eq!(nav.destination_view(), None);
        no_script_errors(&mut cx);
    }

    #[test]
    fn picker_reaches_all_mailboxes_and_move_removes_an_unread_reader() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        view.update_layout(&mut cx, dvec2(874.0, 300.0));
        view.open_sheet(&mut cx, SheetKind::MailboxPicker);
        assert_eq!(view.sheet_actions.len(), 10);
        assert_eq!(view.view.widget(&cx, ids!(panel)).walk(&mut cx).height, Size::Fixed(244.0));
        assert_eq!(view.view.widget(&cx, ids!(panel)).walk(&mut cx).width, Size::Fixed(280.0));
        assert!(matches!(view.view.widget(&cx, ids!(choices)).walk(&mut cx).height, Size::Fill {..}));
        assert!(view.view.widget(&cx, ids!(sheet8)).visible());
        assert!(view.view.widget(&cx, ids!(sheet9)).visible());
        click(&mut view, &mut cx, ids!(sheet9));
        assert_eq!(view.ui.list.mailbox, Mailbox::Folder(Folder::Travel));
        view.open_sheet(&mut cx, SheetKind::MailboxPicker);
        click(&mut view, &mut cx, ids!(sheet8));
        assert_eq!(view.ui.list.mailbox, Mailbox::Folder(Folder::Projects));
        view.select_mailbox(&mut cx, Mailbox::Folder(Folder::Inbox));
        view.ui.list.unread_only = true; view.rebuild_rows();
        view.open_row(&mut cx, MessageId(1));
        assert!(view.keep_reader);
        view.open_sheet(&mut cx, SheetKind::MessageActions);
        assert!(view.sheet_actions.contains(&SheetChoice::MoveTo));
        view.handle_sheet_choice(&mut cx, SheetChoice::MoveTo);
        view.handle_sheet_choice(&mut cx, SheetChoice::MoveFolder(Folder::Archive));
        assert!(view.ui.selected.is_none());
        assert_eq!(view.ui.history.last(), Some(&Route::Messages));
        view.update_layout(&mut cx, dvec2(402.0, 780.0));
        for (id, command) in [(MessageId(3), Command::Archive {id: MessageId(3)}), (MessageId(5), Command::Delete {id: MessageId(5)})] {
            view.open_row(&mut cx, id);
            assert!(view.keep_reader);
            let mutation = view.mutate(&mut cx, command).unwrap();
            view.after_removal(&mut cx, Some(mutation));
            assert!(view.ui.selected.is_none());
            assert_eq!(view.ui.history.last(), Some(&Route::Messages));
        }
        no_script_errors(&mut cx);
    }

    #[test]
    fn rows_constrain_text_and_show_every_attachment_and_search_folder() {
        let (mut cx, root) = root();
        let mut view = root.borrow_mut::<MailView>().unwrap();
        let list = view.view.portal_list(&cx, ids!(wide_list));
        let item = list.item(&mut cx, 0, live_id!(Message));
        for (width, height, row_h, lines, sender_size) in [(402.0, 780.0, 104.0, 2, 12.75), (1240.0, 800.0, 84.0, 2, 9.75), (874.0, 300.0, 76.0, 1, 9.75)] {
            view.update_layout(&mut cx, dvec2(width, height));
            view.bind_message_row(&mut cx, item.clone(), view.rows[0].clone());
            assert_eq!(item.walk(&mut cx).height, Size::Fixed(row_h));
            let preview = item.label(&cx, ids!(preview));
            assert_eq!(preview.borrow().unwrap().max_lines, lines);
            assert!(matches!(preview.borrow().unwrap().walk.width, Size::Fill {..}));
            assert_eq!(item.label(&cx, ids!(sender)).borrow().unwrap().draw_text.text_style.font_size, sender_size);
            assert_eq!(item.label(&cx, ids!(subject)).borrow().unwrap().max_lines, 1);
        }
        view.ui.list.query = "harbor".into(); view.ui.list.scope = SearchScope::AllMail;
        view.rebuild_rows();
        let row = view.rows[0].clone();
        let folder = row.mailbox.label();
        view.bind_message_row(&mut cx, item.clone(), row);
        assert!(item.label(&cx, ids!(preview)).text().starts_with(folder));
        let m = view.doc.as_mut().unwrap().message_mut(MessageId(1)).unwrap();
        m.attachments = (0..8).map(|i| Attachment {name: format!("long attachment {i}.pdf"), mime: "application/pdf".into(), size_bytes: 2000}).collect();
        view.ui.selected = Some(MessageId(1)); view.bind_reader(&mut cx);
        let reader = view.view.widget(&cx, ids!(wide_reader));
        assert!(reader.widget(&cx, ids!(chip7)).visible());
        assert!(reader.label(&cx, ids!(a7)).text().contains("attachment 7"));
        for id in [ids!(r_subject), ids!(r_from), ids!(r_to), ids!(a7)] {
            assert!(matches!(reader.label(&cx, id).borrow().unwrap().walk.width, Size::Fill {..}));
            assert_eq!(reader.label(&cx, id).borrow().unwrap().max_lines, 0);
        }
        view.handle_sheet_choice(&mut cx, SheetChoice::Forward);
        assert!(view.view.widget(&cx, ids!(fchip7)).visible());
        no_script_errors(&mut cx);
    }

    #[test]
    fn secondary_text_uses_muted_role_when_meta_differs_in_light_and_dark_palettes() {
        for muted in [vec4(0.25, 0.30, 0.35, 1.0), vec4(0.75, 0.80, 0.85, 1.0)] {
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.init_cx_os();
            let root = cx.with_vm(|vm| {
                makepad_widgets::script_mod(vm);
                makepad_wm_theme::apply(vm);
                script_eval!(vm, {
                    mod.theme.color_text_muted = #(muted)
                    mod.theme.color_text_meta = #ff00ff
                });
                crate::script_mod(vm);
                let value = script_eval!(vm, {use mod.widgets.* MailView{}});
                WidgetRef::script_from_value(vm, value)
            });
            let view = root.borrow::<MailView>().unwrap();
            let row = view.view.portal_list(&cx, ids!(wide_list)).item(&mut cx, 0, live_id!(Message));
            for label in [row.label(&cx, ids!(date)), row.label(&cx, ids!(preview)), view.view.label(&cx, ids!(wide_counts)), view.view.label(&cx, ids!(compose_from))] {
                assert_eq!(label.borrow().unwrap().draw_text.color, muted);
            }
            let icon = row.widget(&cx, ids!(clip)).script_source();
            cx.with_vm(|vm| {
                let matches = script_eval!(vm, {
                    let clip = #(icon)
                    clip[0].draw_icon.color == mod.theme.color_text_muted
                });
                assert_eq!(matches.as_bool(), Some(true));
            });
            no_script_errors(&mut cx);
        }
    }

    #[test]
    fn glass_and_scope_materials_bind_to_theme_roles() {
        let (mut cx, root) = root();
        let view = root.borrow::<MailView>().unwrap();
        // Inspect evaluated properties, not source substrings or screenshots.
        let panel = view.view.widget(&cx, ids!(wide_toolbar)).script_source();
        let scope = view.view.widget(&cx, ids!(wide_scope)).script_source();
        let action = view.view.widget(&cx, ids!(wide_compose)).script_source();
        cx.with_vm(|vm| {
            let matches = script_eval!(vm, {
                use mod.widgets.*
                let panel = #(panel)
                let segmented = #(scope)
                let action = #(action)
                panel.draw_bg.fallback_color == mod.theme.color_inset
                    && segmented.draw_text.color == mod.theme.color_text
                    && segmented.draw_sel.fill_color == mod.theme.color_bg_highlight
                    && action.draw_glass.tint == mod.theme.color_inset
            });
            assert_eq!(matches.as_bool(), Some(true));
        });
        no_script_errors(&mut cx);
    }
}
