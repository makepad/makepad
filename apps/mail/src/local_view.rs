//! Alternate view over a worker-owned mail source and search cache.
use crate::{
    mail_index::{mailbox_kind, MailIndex, MailboxSummary, MessageSummary, SearchPage},
    mail_worker::{MailWorker, Reply, Request},
    source::LocalConfig,
};
use makepad_widgets::*;
use std::{
    collections::{BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    mod.widgets.LocalMailViewBase = #(LocalMailView::register_widget(vm))
    // One palette, derived from the theme's roles so light and dark both
    // follow the system appearance: a slightly shaded sidebar, content on
    // the app background, hairline rules, and selection as an accent tint
    // that keeps the normal text colour readable.
    let secondary = #(crate::view::secondary_text_color(vm))
    let sidebar_bg = mix(theme.color_bg_app, theme.color_text, 0.035)
    let selection = mix(theme.color_bg_app, theme.color_focus, 0.26)
    let field_bg = mix(theme.color_bg_app, theme.color_text, 0.07)
    let hairline = mix(theme.color_bg_app, theme.color_text, 0.10)
    let Text = Label{width: Fill height: Fit padding: 0 draw_text +: {color: theme.color_text wrap: Words text_style: theme.font_regular{font_size: 10.5}}}
    let Muted = Text{draw_text +: {color: secondary}}
    let Selectable = Html{width: Fill height: Fit padding: 0 selectable: true font_size: 10.5 font_color: theme.color_text paragraph_margin: Inset{top: 0 bottom: 0}}
    let Strong = Text{draw_text +: {text_style: theme.font_bold{font_size: 10.5}}}
    let Rule = SolidView{width: Fill height: 1 draw_bg.color: hairline}
    let VRule = SolidView{width: 1 height: Fill draw_bg.color: hairline}
    let Glyph = Icon{width: 15 height: 15 icon_walk: Walk{width: 15 height: 15} draw_icon +: {color: secondary}}
    let QuietButton = ButtonFlat{
        margin: 0 padding: Inset{left: 7 right: 7 top: 3 bottom: 3} height: 26 spacing: 5
        draw_text +: {color: secondary color_hover: theme.color_text color_focus: theme.color_text text_style: theme.font_regular{font_size: 9.5}}
        draw_icon +: {color: secondary color_hover: theme.color_text}
        icon_walk: Walk{width: 14 height: 14}
        draw_bg +: {border_size: 0 border_radius: 5 color: #0000 color_hover: field_bg color_down: selection color_focus: #0000}
    }
    let AttachmentRow = QuietButton{
        visible: false width: Fill height: 26 align: Align{x: 0.0 y: 0.5}
        draw_text +: {color: theme.color_text}
        draw_icon.svg: crate_resource("self:resources/icons/outline_attachment.svg")
    }
    let MailScrollBar = ScrollBar{bar_size: 8 bar_side_margin: 2 draw_bg +: {size: 4 border_size: 0 border_radius: 2 color: mix(theme.color_bg_app, theme.color_text, 0.22) color_hover: secondary color_drag: secondary}}
    let Hit = ButtonFlat{
        width: Fill height: Fill text: "" margin: 0 padding: 0
        draw_bg +: {pixel: fn() {return vec4(0.0, 0.0, 0.0, 0.0)}}
    }
    // Sidebar rows: a fixed disclosure slot (so every icon lines up at a
    // depth), an accent glyph at one optical size, the name, and a quiet
    // count on the right edge.
    let FolderRow = View{
        width: Fill height: 24 flow: Overlay
        selected_bg := RoundedView{visible: false width: Fill height: Fill draw_bg +: {color: selection border_radius: 5}}
        hit := Hit{}
        contents := View{width: Fill height: Fill flow: Right spacing: 5 align: Align{y: 0.5} padding: Inset{left: 4 right: 8}
            disclosure := View{width: 12 height: 24 flow: Overlay align: Align{x: 0.5 y: 0.5}
                toggle_closed := QuietButton{visible: false width: 12 height: 20 padding: 0 text: "" icon_walk: Walk{width: 9 height: 9} draw_icon +: {svg: crate_resource("self:resources/icons/chevron.svg")} draw_bg +: {color_hover: #0000 color_down: #0000}}
                toggle_open := QuietButton{visible: false width: 12 height: 20 padding: 0 text: "" icon_walk: Walk{width: 9 height: 9} draw_icon +: {svg: crate_resource("self:resources/icons/chevron_down.svg")} draw_bg +: {color_hover: #0000 color_down: #0000}}
            }
            glyph := Glyph{draw_icon +: {color: theme.color_focus svg: crate_resource("self:resources/icons/outline_inbox.svg")}}
            name := Text{height: Fit max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 10.5}}
            count := Muted{width: Fit height: Fit draw_text.text_style: theme.font_regular{font_size: 9}}
        }
    }
    let FolderHeading = View{width: Fill height: 26 flow: Down align: Align{y: 1.0} padding: Inset{left: 8 bottom: 4}
        name := Muted{draw_text.text_style: theme.font_bold{font_size: 8.5}}
    }
    // Message rows: sender and a compact date on one line, subject, two
    // lines of preview; a hairline between rows, inset like Mail's. The
    // lines size to their text (about 17 px each at these sizes), so the
    // preview's second line ends in an ellipsis instead of being cut; the
    // row height holds exactly the four lines.
    let Message = View{
        width: Fill height: 88 flow: Overlay
        sel := RoundedView{visible: false width: Fill height: Fill margin: Inset{left: 6 right: 6 top: 1 bottom: 1} draw_bg +: {color: selection border_radius: 6}}
        hit := Hit{}
        contents := View{width: Fill height: Fill flow: Down padding: Inset{left: 18 right: 14 top: 9 bottom: 7} spacing: 1
            View{width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                sender := Strong{max_lines: 1 text_overflow: Ellipsis}
                date := Muted{width: Fit max_lines: 1 draw_text.text_style: theme.font_regular{font_size: 9}}
            }
            subject := Text{height: Fit max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 10}}
            View{width: Fill height: Fit flow: Right spacing: 4
                preview := Muted{height: Fit max_lines: 2 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 9.5}}
                clip := View{visible: false width: 13 height: 13 Glyph{width: 13 height: 13 icon_walk: Walk{width: 13 height: 13} draw_icon.svg: crate_resource("self:resources/icons/outline_attachment.svg")}}
            }
        }
        View{width: Fill height: Fill flow: Down align: Align{y: 1.0} padding: Inset{left: 18}
            Rule{}
        }
    }
    mod.widgets.LocalMailView = set_type_default() do mod.widgets.LocalMailViewBase{
        width: Fill height: Fill flow: Down show_bg: true draw_bg.color: theme.color_bg_app
        // One toolbar band whose parts sit over their columns (the widths
        // follow the columns, see `layout_columns`): search over the
        // sidebar, the mailbox title over the list, the mailbox path and
        // message actions over the reader.
        toolbar := View{width: Fill height: 44 flow: Right align: Align{y: 0.5} spacing: 0
            search_area := View{width: 220 height: Fill flow: Right align: Align{y: 0.5} padding: Inset{left: 10 right: 10} show_bg: true draw_bg.color: sidebar_bg
                search_shell := RoundedView{width: Fill height: 26 flow: Right spacing: 5 align: Align{y: 0.5} padding: Inset{left: 8 right: 3} draw_bg +: {color: field_bg border_radius: 6}
                    Glyph{width: 13 height: 13 icon_walk: Walk{width: 13 height: 13} draw_icon.svg: crate_resource("self:resources/icons/search.svg")}
                    search := TextInput{width: Fill height: 26 margin: 0 padding: Inset{left: 0 top: 5 right: 0 bottom: 5} is_multiline: false empty_text: "Search" draw_bg +: {border_size: 0 border_radius: 0 color: #0000 color_hover: #0000 color_focus: #0000 color_down: #0000 color_empty: #0000 color_disabled: #0000 color_2: #0000 color_2_hover: #0000 color_2_focus: #0000 color_2_down: #0000 color_2_empty: #0000 color_2_disabled: #0000 border_color: #0000 border_color_hover: #0000 border_color_focus: #0000 border_color_down: #0000 border_color_empty: #0000 border_color_disabled: #0000 border_color_2: #0000 border_color_2_hover: #0000 border_color_2_focus: #0000 border_color_2_down: #0000 border_color_2_empty: #0000 border_color_2_disabled: #0000} draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10}}}
                    clear_search := QuietButton{visible: false width: 20 height: 20 padding: 0 text: "×"}
                }
            }
            VRule{}
            list_header := View{width: 330 height: Fill flow: Down align: Align{y: 0.5} spacing: 1 padding: Inset{left: 18 right: 12}
                title := Strong{text: "All Mail" max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_bold{font_size: 12}}
                counts := Muted{text: "All downloaded messages" max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 9}}
            }
            VRule{}
            reader_header := View{width: Fill height: Fill flow: Right align: Align{y: 0.5} spacing: 2 padding: Inset{left: 16 right: 10}
                mailbox := Muted{text: "" max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 9}}
                reply := QuietButton{visible: false text: "Reply" draw_icon.svg: crate_resource("self:resources/icons/outline_reply.svg")}
                open_mail := QuietButton{visible: false text: "Open in Mail" draw_icon.svg: crate_resource("self:resources/icons/outline_open.svg")}
                View{width: 10 height: 16 align: Align{x: 0.5} VRule{height: 16}}
                refresh := QuietButton{text: "" width: 28 padding: Inset{left: 7 right: 7} draw_icon.svg: crate_resource("self:resources/icons/refresh.svg")}
                help_button := QuietButton{text: "Tips"}
            }
        }
        Rule{}
        help := SolidView{visible: false width: Fill height: Fit flow: Down spacing: 3 padding: Inset{left: 16 right: 16 top: 10 bottom: 10} show_bg: true draw_bg.color: field_bg
            Strong{text: "Search tips"}
            Muted{text: "Try from:alex, subject:invoice, has:attachment, after:2026-01-01, or an \"exact phrase\". Put - before a word to exclude it. Search applies within the selected mailbox."}
        }
        permissions := View{visible: false width: Fill height: Fit flow: Down spacing: 8 padding: 16
            permission_text := Text{}
            View{width: Fill height: Fit flow: Right spacing: 8
                settings := Button{text: "Open Privacy Settings" height: 28}
                retry := Button{text: "Try Again" height: 28}
                reimport := Button{text: "Reimport" height: 28}
            }
        }
        content := View{width: Fill height: Fill flow: Right
            sidebar := View{width: 220 height: Fill flow: Down show_bg: true draw_bg.color: sidebar_bg
                folders := PortalList{scroll_bar: MailScrollBar{} width: Fill height: Fill margin: Inset{left: 6 right: 6 top: 2} Folder := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_projects.svg")}}}}
                    Inbox := FolderRow{}
                    Sent := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_sent.svg")}}}}
                    Drafts := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_drafts.svg")}}}}
                    Archive := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_archive.svg")}}}}
                    Trash := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_trash.svg")}}}}
                    Attachment := FolderRow{contents +: {glyph +: {draw_icon +: {svg: crate_resource("self:resources/icons/outline_attachment.svg")}}}}
                    Heading := FolderHeading{}}
                Rule{}
                sidebar_footer := View{width: Fill height: Fit flow: Down padding: Inset{left: 14 right: 12 top: 7 bottom: 8} spacing: 2
                    View{width: Fill height: Fit flow: Right spacing: 5 align: Align{y: 0.5}
                        RoundedView{width: 5 height: 5 draw_bg +: {color: theme.color_success border_radius: 2.5}}
                        account_status := Muted{text: "On this Mac" draw_text.text_style: theme.font_regular{font_size: 8.5}}
                    }
                    status := Muted{text: "Opening downloaded mail…" max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 8.5}}
                }
            }
            VRule{}
            list_column := View{width: 330 height: Fill flow: Down
                messages := PortalList{scroll_bar: MailScrollBar{} width: Fill height: Fill Message := Message{}}
                list_empty := View{visible: false width: Fill height: Fit flow: Down padding: 18 spacing: 4
                    Strong{text: "No messages found"}
                    Muted{text: "Try a different search or choose another mailbox."}
                }
            }
            VRule{}
            reader_column := View{width: Fill height: Fill flow: Down
                reader_placeholder := View{width: Fill height: Fill flow: Down align: Align{x: 0.5 y: 0.45} spacing: 8 padding: 28
                    Icon{width: 34 height: 34 icon_walk: Walk{width: 34 height: 34} draw_icon +: {svg: crate_resource("self:resources/icons/outline_inbox.svg") color: secondary}}
                    Strong{width: Fit text: "No Message Selected" draw_text.text_style: theme.font_bold{font_size: 12}}
                }
                reader := ScrollYView{scroll_bars +: {scroll_bar_y: MailScrollBar{}} visible: false width: Fill height: Fill flow: Down
                    reader_content := View{width: Fill height: Fit flow: Down spacing: 12 padding: Inset{left: 24 right: 24 top: 16 bottom: 32}
                        View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.0}
                            avatar := RoundedView{width: 32 height: 32 align: Align{x: 0.5 y: 0.5} draw_bg +: {color: selection border_radius: 16}
                                initials := Strong{width: Fit draw_text +: {color: theme.color_focus text_style: theme.font_bold{font_size: 11}}}
                            }
                            View{width: Fill height: Fit flow: Down spacing: 3
                                View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.0}
                                    from := Selectable{}
                                    reader_date := Muted{width: Fit max_lines: 1 draw_text.text_style: theme.font_regular{font_size: 9}}
                                }
                                to := Selectable{font_size: 9.5 font_color: secondary}
                                // A narrow reader puts the date on its own line (see
                                // `layout_columns`), leaving the sender the row.
                                reader_date_below := Muted{visible: false width: Fill max_lines: 1 draw_text.text_style: theme.font_regular{font_size: 9}}
                            }
                        }
                        subject := Selectable{font_size: 14}
                        Rule{}
                        body := Html{selectable: true width: Fill height: Fit padding: 0 font_size: 11 font_color: theme.color_text paragraph_margin: Inset{top: 0.3 bottom: 0.5} draw_text.color: theme.color_text
                            a := HtmlLink{color: theme.color_focus hover_color: theme.color_text pressed_color: theme.color_focus}
                        }
                        attachments := View{visible: false width: Fill height: Fit flow: Down spacing: 6
                            Rule{}
                            Muted{text: "Attachments" draw_text.text_style: theme.font_bold{font_size: 9}}
                            RoundedView{width: Fill height: Fit flow: Down spacing: 0 padding: 4 draw_bg +: {color: field_bg border_radius: 6}
                                att0 := AttachmentRow{}
                                att1 := AttachmentRow{}
                                att2 := AttachmentRow{}
                                att3 := AttachmentRow{}
                                att4 := AttachmentRow{}
                                att5 := AttachmentRow{}
                                att6 := AttachmentRow{}
                                att7 := AttachmentRow{}
                                att8 := AttachmentRow{}
                                att9 := AttachmentRow{}
                                att10 := AttachmentRow{}
                                att11 := AttachmentRow{}
                                att12 := AttachmentRow{}
                                att13 := AttachmentRow{}
                                att14 := AttachmentRow{}
                                att15 := AttachmentRow{}
                                attachment_more := Muted{visible: false padding: Inset{left: 8 top: 4 bottom: 4}}
                            }
                            attachment_status := Muted{visible: false}
                        }
                        coverage := Muted{visible: false draw_text.text_style: theme.font_regular{font_size: 9.5}}
                    }
                }
            }
        }
    }
}

/// A worker restart that waits for the previous worker to release the cache.
struct Restart {
    /// Delete the cache before starting, so the source is imported again.
    wipe: bool,
}

#[derive(Script, ScriptHook, Widget)]
pub struct LocalMailView {
    #[deref]
    view: View,
    #[rust]
    config: Option<LocalConfig>,
    #[rust]
    worker: Option<MailWorker>,
    /// Finished flag of the last retired worker; a restart waits on it.
    #[rust]
    retired: Option<Arc<AtomicBool>>,
    /// A restart waiting for the retired worker to release the cache.
    #[rust]
    restart: Option<Restart>,
    #[rust]
    pending: VecDeque<Request>,
    #[rust]
    page: Arc<SearchPage>,
    #[rust]
    selected: Option<u64>,
    #[rust]
    query: String,
    #[rust]
    generation: u64,
    #[rust]
    timer: Timer,
    #[rust]
    status: String,
    #[rust]
    started: bool,
    #[rust]
    reset_list: bool,
    #[rust]
    folder_rows: Vec<FolderItem>,
    #[rust]
    mailbox_catalog: Arc<Vec<MailboxSummary>>,
    #[rust]
    collapsed: BTreeSet<String>,
    #[rust]
    filter: String,
    #[rust]
    filter_title: String,
    #[rust]
    selected_folder: String,
    #[rust]
    selected_message: Option<Arc<MessageSummary>>,
    #[rust]
    help_open: bool,
    /// What the list shows for All Mail (no mailbox, no query), once a
    /// search has said so: the sidebar's All Mail count uses the same
    /// figure, not the raw number of indexed records.
    #[rust]
    all_mail_total: Option<usize>,
    /// The (sidebar, list) widths and the narrow-reader choice last
    /// applied, see `layout_columns`.
    #[rust]
    columns: (f64, f64, bool),
}
impl LocalMailView {
    pub fn configure(&mut self, config: LocalConfig) {
        self.config = Some(config);
    }
    pub fn shutdown(&mut self) {
        self.worker = None;
    }
    fn retire_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.retired = Some(worker.retire());
        }
    }
    /// Stop the current worker and start a fresh one once it has let go of
    /// the cache. `wipe` deletes the cache first, so the source is imported
    /// again from scratch.
    fn restart(&mut self, cx: &mut Cx, wipe: bool) {
        if self.restart.is_some() {
            return;
        }
        self.retire_worker();
        self.pending.clear();
        self.restart = Some(Restart { wipe });
        self.status = if wipe {
            "Removing the search cache…".into()
        } else {
            "Reopening mail…".into()
        };
        self.view.label(cx, ids!(status)).set_text(cx, &self.status);
        self.poll_restart(cx);
    }
    fn poll_restart(&mut self, cx: &mut Cx) {
        let Some(restart) = &self.restart else { return };
        if self.retired.as_ref().is_some_and(|f| !f.load(Ordering::Acquire)) {
            return;
        }
        let wipe = restart.wipe;
        self.restart = None;
        self.retired = None;
        if wipe {
            if let Some(config) = &self.config {
                if let Err(error) = MailIndex::remove_cache(&config.cache) {
                    self.show_error(cx, &format!("Cannot remove the search cache. {error}"));
                    return;
                }
                log!("mail: search cache removed for reimport");
            }
        }
        cx.stop_timer(self.timer);
        self.start(cx);
    }
    fn start(&mut self, cx: &mut Cx) {
        let Some(config) = self.config.clone() else {
            return;
        };
        self.started = true;
        self.reset_list = true;
        self.filter_title = "All Mail".into();
        self.rebuild_folders();
        self.timer = cx.start_interval(0.05);
        match MailWorker::spawn(cx, config) {
            Ok(worker) => {
                self.worker = Some(worker);
                self.view
                    .widget(cx, ids!(permissions))
                    .set_visible(cx, false);
                self.queue_search();
            }
            Err(error) => self.show_error(cx, &error),
        }
    }
    fn show_error(&mut self, cx: &mut Cx, error: &str) {
        log!("mail: displaying source access error");
        self.status = "Mail source unavailable".into();
        self.view
            .widget(cx, ids!(permissions))
            .set_visible(cx, true);
        self.view
            .label(cx, ids!(permission_text))
            .set_text(cx, error);
        self.view.label(cx, ids!(status)).set_text(cx, &self.status);
    }
    fn queue_search(&mut self) {
        self.generation += 1;
        if let Some(worker) = &self.worker {
            worker.generation.store(self.generation, Ordering::Release);
        }
        self.pending
            .retain(|r| !matches!(r, Request::Search { .. }));
        self.pending.push_back(Request::Search {
            generation: self.generation,
            text: format!("{} {}", self.filter, self.query).trim().to_owned(),
            // Every match: the list holds the whole result and draws only the
            // rows on screen. The page shares the index's summaries, so this
            // costs a pointer per message, not a copy.
            limit: usize::MAX,
        });
    }
    fn drain(&mut self, cx: &mut Cx) {
        while let Some(request) = self.pending.pop_front() {
            let Some(worker) = &self.worker else { break };
            if let Err(request) = worker.send(request) {
                self.pending.push_front(request);
                self.view
                    .label(cx, ids!(status))
                    .set_text(cx, "Search worker busy · retrying…");
                break;
            }
        }
        let replies: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| w.rx.try_iter().collect())
            .unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Status(status) | Reply::ScanDone(status) => {
                    self.status = status;
                    let text = if self.status.starts_with("Indexing") || self.status.starts_with("Opening") {
                        self.status.clone()
                    } else {
                        format!("{} messages indexed", grouped(self.page.indexed))
                    };
                    self.view.label(cx, ids!(status)).set_text(cx, &text);
                }
                Reply::Error(error) => {
                    self.retire_worker();
                    self.show_error(cx, &error);
                }
                Reply::QueryError {
                    generation,
                    message,
                } if generation == self.generation => {
                    self.view.label(cx, ids!(counts)).set_text(cx, &message);
                }
                Reply::Results { generation, page } if generation == self.generation => {
                    let mut folder_change = !Arc::ptr_eq(&self.mailbox_catalog, &page.mailboxes);
                    self.page = page;
                    if self.filter.is_empty() && self.query.is_empty() && self.all_mail_total != Some(self.page.total) {
                        self.all_mail_total = Some(self.page.total);
                        folder_change = true;
                    }
                    if folder_change {
                        self.mailbox_catalog = self.page.mailboxes.clone();
                        self.rebuild_folders();
                    }
                    self.view.label(cx, ids!(counts)).set_text(
                        cx,
                        &format!(
                            "{} {}{}",
                            grouped(self.page.total),
                            if self.page.total == 1 { "message" } else { "messages" },
                            if self.query.is_empty() { "" } else { " found" }
                        ),
                    );
                    self.view.label(cx, ids!(title)).set_text(
                        cx,
                        if self.query.is_empty() {
                            &self.filter_title
                        } else {
                            "Search Results"
                        },
                    );
                    self.view
                        .widget(cx, ids!(list_empty))
                        .set_visible(cx, self.page.hits.is_empty());
                    self.view
                        .widget(cx, ids!(messages))
                        .set_visible(cx, !self.page.hits.is_empty());
                    if !self.status.starts_with("Indexing") {
                        self.view.label(cx, ids!(status)).set_text(
                            cx,
                            &format!("{} messages indexed", grouped(self.page.indexed)),
                        );
                    }
                    if self.reset_list {
                        if let Some(mut list) =
                            self.view.portal_list(cx, ids!(messages)).borrow_mut()
                        {
                            list.set_first_id_and_scroll(0, 0.0);
                        }
                        self.reset_list = false;
                    }
                    self.view.redraw(cx);
                }
                Reply::Body { id, text } if self.selected == Some(id) => {
                    self.view.html(cx, ids!(body)).set_text(cx, &text);
                    self.view.redraw(cx);
                }
                Reply::Attachment { id, path, .. } if self.selected == Some(id) => {
                    log!("mail: opening attachment {}", path.display());
                    self.view
                        .widget(cx, ids!(attachment_status))
                        .set_visible(cx, false);
                    cx.open_url(&path.to_string_lossy(), OpenUrlInPlace::Yes);
                }
                Reply::AttachmentError { id, message, .. } if self.selected == Some(id) => {
                    self.view
                        .widget(cx, ids!(attachment_status))
                        .set_visible(cx, true);
                    self.view
                        .label(cx, ids!(attachment_status))
                        .set_text(cx, &message);
                }
                _ => {}
            }
        }
    }
    fn select(&mut self, cx: &mut Cx, message: Arc<MessageSummary>) {
        self.selected = Some(message.id);
        for path in [ids!(subject), ids!(from), ids!(to), ids!(body)] {
            if let Some(mut html) = self.view.html(cx, path).borrow_mut() {
                html.text_flow.clear_selection();
            }
        }
        self.selected_message = Some(message.clone());
        self.view
            .widget(cx, ids!(reader_placeholder))
            .set_visible(cx, false);
        self.view.widget(cx, ids!(reader)).set_visible(cx, true);
        self.view.widget(cx, ids!(reply)).set_visible(cx, true);
        self.view
            .widget(cx, ids!(open_mail))
            .set_visible(cx, !message.message_id.is_empty());
        self.view
            .label(cx, ids!(initials))
            .set_text(cx, &initials(&display_sender(&message.from)));
        self.view
            .view(cx, ids!(reader))
            .set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.view.html(cx, ids!(subject)).set_text(cx, &format!("<b>{}</b>", crate::presentation::plain(&message.subject)));
        self.view.html(cx, ids!(from)).set_text(cx, &format!("<b>{}</b>", crate::presentation::plain(&message.from)));
        self.view
            .html(cx, ids!(to))
            .set_text(cx, &crate::presentation::plain(&format!("To: {}", message.to)));
        let date = reader_date(message.date);
        self.view.label(cx, ids!(reader_date)).set_text(cx, &date);
        self.view.label(cx, ids!(reader_date_below)).set_text(cx, &date);
        let mut place = message.mailboxes.join(" / ");
        for label in &message.labels {
            place.push_str(" · ");
            place.push_str(label);
        }
        self.view.label(cx, ids!(mailbox)).set_text(cx, &place);
        self.view
            .widget(cx, ids!(attachments))
            .set_visible(cx, !message.attachments.is_empty());
        let rows = attachment_rows();
        for (i, row) in rows.iter().enumerate() {
            match message.attachments.get(i) {
                Some(name) => {
                    self.view.button(cx, row).set_text(cx, name);
                    self.view.widget(cx, row).set_visible(cx, true);
                }
                None => self.view.widget(cx, row).set_visible(cx, false),
            }
        }
        let extra = message.attachments.len().saturating_sub(rows.len());
        self.view
            .widget(cx, ids!(attachment_more))
            .set_visible(cx, extra > 0);
        self.view.label(cx, ids!(attachment_more)).set_text(
            cx,
            &format!("{extra} more. Open this message in Mail to see them all."),
        );
        self.view
            .widget(cx, ids!(attachment_status))
            .set_visible(cx, false);
        self.view
            .widget(cx, ids!(coverage))
            .set_visible(cx, message.incomplete);
        self.view.label(cx, ids!(coverage)).set_text(
            cx,
            "Some content hasn’t been downloaded. Open this message in Mail to download it.",
        );
        self.view
            .html(cx, ids!(body))
            .set_text(cx, "Loading message…");
        self.pending.retain(|r| !matches!(r, Request::Read(_)));
        self.pending.push_back(Request::Read(message.id));
        self.view.redraw(cx);
    }
    /// Column widths for the window: the sidebar and list take a share of
    /// it within readable bounds, the reader the rest; the toolbar's parts
    /// follow so they sit over their columns.
    fn layout_columns(&mut self, cx: &mut Cx, width: f64) {
        let sidebar = (width * 0.16).clamp(170.0, 230.0).round();
        let list = (width * 0.25).clamp(240.0, 350.0).round();
        // Below about 560 px of reader (a 1000 px window and narrower), the
        // full date gets its own line and Open in Mail shows its icon only,
        // so the sender and the mailbox path keep their room.
        let narrow = width - sidebar - list - 2.0 < 560.0;
        if self.columns == (sidebar, list, narrow) {
            return;
        }
        self.columns = (sidebar, list, narrow);
        for (path, w) in [(ids!(search_area), sidebar), (ids!(sidebar), sidebar), (ids!(list_header), list), (ids!(list_column), list)] {
            if let Some(mut view) = self.view.view(cx, path).borrow_mut() {
                view.walk.width = Size::Fixed(w);
            }
        }
        self.view.widget(cx, ids!(reader_date)).set_visible(cx, !narrow);
        self.view.widget(cx, ids!(reader_date_below)).set_visible(cx, narrow);
        self.view.button(cx, ids!(open_mail)).set_text(cx, if narrow { "" } else { "Open in Mail" });
    }
    fn rebuild_folders(&mut self) {
        let mut rows = vec![
            FolderItem::heading("Favorites"),
            FolderItem::smart("", "All Mail", "", live_id!(Inbox), self.all_mail_total.unwrap_or(self.page.indexed)),
        ];
        for (key, name, filter, template) in [
            ("inbox", "All Inboxes", "kind:inbox", live_id!(Inbox)),
            ("sent", "All Sent", "kind:sent", live_id!(Sent)),
            ("draft", "All Drafts", "kind:draft", live_id!(Drafts)),
        ] {
            let count = self
                .mailbox_catalog
                .iter()
                .filter(|m| m.path.last().is_some_and(|p| mailbox_kind(p) == Some(key)))
                .map(|m| m.count)
                .sum();
            rows.push(FolderItem::smart(key, name, filter, template, count));
        }
        rows.push(FolderItem::heading("Smart Mailboxes"));
        rows.push(FolderItem::smart(
            "attachments",
            "Attachments",
            "has:attachment",
            live_id!(Attachment),
            0,
        ));
        rows.push(FolderItem::heading("On My Mac"));
        let parents: BTreeSet<&[String]> = self.mailbox_catalog.iter()
            .filter(|m| m.path.len() > 1)
            .map(|m| &m.path[..m.path.len()-1]).collect();
        for mailbox in self.mailbox_catalog.iter() {
            let path = &mailbox.path;
            if (1..path.len()).any(|n| self.collapsed.contains(&path[..n].join(" / "))) {
                continue;
            }
            let key = path.join(" / ");
            let name = path.last().cloned().unwrap_or_default();
            let expandable = parents.contains(path.as_slice());
            let template = match mailbox_kind(&name) {
                Some("inbox") => live_id!(Inbox),
                Some("sent") => live_id!(Sent),
                Some("draft") => live_id!(Drafts),
                Some("archive") => live_id!(Archive),
                Some("trash") | Some("spam") => live_id!(Trash),
                _ => live_id!(Folder),
            };
            rows.push(FolderItem {
                filter: format!("folder:{}", quote_query(&key)),
                key,
                name: pretty_folder(&name),
                count: mailbox.count,
                depth: path.len() - 1,
                expandable,
                heading: false,
                template,
            });
        }
        self.folder_rows = rows;
    }
    fn actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(action) = action.as_widget_action() {
                if let HtmlLinkAction::Clicked { url, .. } = action.cast() {
                    if ["https://", "http://", "mailto:"].iter().any(|scheme| url.to_ascii_lowercase().starts_with(scheme)) {
                        cx.open_url(&url, OpenUrlInPlace::Yes);
                    }
                }
            }
        }
        if let Some(text) = self.view.text_input(cx, ids!(search)).changed(actions) {
            self.query = text;
            self.view
                .widget(cx, ids!(clear_search))
                .set_visible(cx, !self.query.is_empty());
            self.reset_list = true;
            self.queue_search();
        }
        if self.view.button(cx, ids!(clear_search)).clicked(actions) {
            self.query.clear();
            self.view.text_input(cx, ids!(search)).set_text(cx, "");
            self.view
                .widget(cx, ids!(clear_search))
                .set_visible(cx, false);
            self.reset_list = true;
            self.queue_search();
        }
        if self.view.button(cx, ids!(help_button)).clicked(actions) {
            self.help_open = !self.help_open;
            self.view
                .widget(cx, ids!(help))
                .set_visible(cx, self.help_open);
        }
        for (index, item) in self
            .view
            .portal_list(cx, ids!(folders))
            .items_with_actions(actions)
        {
            let Some(row) = self.folder_rows.get(index).cloned() else {
                continue;
            };
            if row.heading {
                continue;
            }
            if row.expandable && (item.button(cx, ids!(toggle_closed)).clicked(actions) || item.button(cx, ids!(toggle_open)).clicked(actions)) {
                if !self.collapsed.remove(&row.key) {
                    self.collapsed.insert(row.key);
                }
                self.rebuild_folders();
                self.view.redraw(cx);
                break;
            }
            if item.button(cx, ids!(hit)).clicked(actions) {
                self.filter = row.filter;
                self.filter_title = row.name;
                self.selected_folder = row.key;
                self.reset_list = true;
                self.queue_search();
                self.view.redraw(cx);
                break;
            }
        }
        if let Some(message) = &self.selected_message {
            if self.view.button(cx, ids!(open_mail)).clicked(actions) {
                cx.open_url(
                    &format!("message:{}", uri_component(&message.message_id)),
                    OpenUrlInPlace::Yes,
                );
            }
            if self.view.button(cx, ids!(reply)).clicked(actions) {
                let address = message
                    .from
                    .split_once('<')
                    .map(|(_, s)| s.trim_end_matches('>'))
                    .unwrap_or(&message.from);
                let subject = if message.subject.to_lowercase().starts_with("re:") {
                    message.subject.clone()
                } else {
                    format!("Re: {}", message.subject)
                };
                cx.open_url(
                    &format!(
                        "mailto:{}?subject={}",
                        uri_component(address),
                        uri_component(&subject)
                    ),
                    OpenUrlInPlace::Yes,
                );
            }
        }
        if self.view.button(cx, ids!(refresh)).clicked(actions) {
            self.pending.retain(|r| !matches!(r, Request::Refresh));
            self.pending.push_back(Request::Refresh);
        }
        if let Some(id) = self.selected {
            for (i, row) in attachment_rows().iter().enumerate() {
                if !self.view.button(cx, row).clicked(actions) {
                    continue;
                }
                let name = self
                    .selected_message
                    .as_ref()
                    .and_then(|m| m.attachments.get(i))
                    .cloned()
                    .unwrap_or_default();
                self.view
                    .widget(cx, ids!(attachment_status))
                    .set_visible(cx, true);
                self.view
                    .label(cx, ids!(attachment_status))
                    .set_text(cx, &format!("Opening {name}…"));
                self.pending
                    .retain(|r| !matches!(r, Request::Attachment { .. }));
                self.pending.push_back(Request::Attachment { id, index: i });
            }
        }
        if self.view.button(cx, ids!(retry)).clicked(actions) {
            self.restart(cx, false);
        }
        if self.view.button(cx, ids!(reimport)).clicked(actions) {
            self.restart(cx, true);
        }
        if self.view.button(cx, ids!(settings)).clicked(actions) {
            cx.open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
                OpenUrlInPlace::Yes,
            );
        }
        let list = self.view.portal_list(cx, ids!(messages));
        let selected = list
            .items_with_actions(actions)
            .into_iter()
            .find_map(|(index, item)| {
                item.button(cx, ids!(hit))
                    .clicked(actions)
                    .then(|| self.page.hits.get(index).cloned())
                    .flatten()
            });
        if let Some(message) = selected {
            self.select(cx, message);
        }
    }
}
impl Widget for LocalMailView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.started {
            return;
        }
        self.poll_restart(cx);
        self.drain(cx);
        if let Event::KeyDown(key) = event {
            if key.key_code == KeyCode::KeyF && (key.modifiers.logo || key.modifiers.control) {
                if let Some(mut input) = self.view.text_input(cx, ids!(search)).borrow_mut() {
                    input.take_key_focus(cx);
                    input.select_all(cx);
                }
            }
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            self.actions(cx, actions);
        }
    }
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.started {
            self.start(cx);
        }
        let width = cx.turtle().inner_size().x;
        if width.is_finite() && width > 0.0 {
            self.layout_columns(cx, width);
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let is_folders =
                step.widget_uid() == self.view.portal_list(cx, ids!(folders)).widget_uid();
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                if is_folders {
                    list.set_item_range(cx, 0, self.folder_rows.len());
                    while let Some(index) = list.next_visible_item(cx) {
                        let Some(row) = self.folder_rows.get(index) else {
                            continue;
                        };
                        let item = list.item(
                            cx,
                            index,
                            if row.heading {
                                live_id!(Heading)
                            } else {
                                row.template
                            },
                        );
                        item.label(cx, ids!(name)).set_text(cx, &row.name);
                        if !row.heading {
                            item.label(cx, ids!(count)).set_text(
                                cx,
                                &if row.count == 0 {
                                    String::new()
                                } else {
                                    grouped(row.count)
                                },
                            );
                            item.widget(cx, ids!(selected_bg))
                                .set_visible(cx, row.key == self.selected_folder);
                            if let Some(mut view) = item.view(cx, ids!(contents)).borrow_mut() {
                                view.layout.padding.left = 6.0 + row.depth as f64 * 14.0;
                            }
                            let collapsed = self.collapsed.contains(&row.key);
                            item.widget(cx, ids!(toggle_closed)).set_visible(cx, row.expandable && collapsed);
                            item.widget(cx, ids!(toggle_open)).set_visible(cx, row.expandable && !collapsed);
                        }
                        item.draw_all(cx, &mut Scope::empty());
                    }
                    continue;
                }
                list.set_item_range(cx, 0, self.page.hits.len());
                let now = now_secs();
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(message) = self.page.hits.get(index) else {
                        continue;
                    };
                    let item = list.item(cx, index, live_id!(Message));
                    item.label(cx, ids!(sender))
                        .set_text(cx, &display_sender(&message.from));
                    item.label(cx, ids!(subject)).set_text(cx, &message.subject);
                    item.label(cx, ids!(preview)).set_text(cx, &message.preview);
                    item.label(cx, ids!(date))
                        .set_text(cx, &list_date(message.date, now));
                    item.widget(cx, ids!(sel))
                        .set_visible(cx, self.selected == Some(message.id));
                    item.widget(cx, ids!(clip))
                        .set_visible(cx, !message.attachments.is_empty());
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }
}

#[derive(Clone)]
struct FolderItem {
    key: String,
    name: String,
    filter: String,
    count: usize,
    depth: usize,
    expandable: bool,
    heading: bool,
    template: LiveId,
}
impl FolderItem {
    fn heading(name: &str) -> Self {
        Self {
            heading: true,
            ..Self::smart(name, name, "", live_id!(Heading), 0)
        }
    }
    fn smart(key: &str, name: &str, filter: &str, template: LiveId, count: usize) -> Self {
        Self {
            key: key.into(),
            name: name.into(),
            filter: filter.into(),
            template,
            count,
            depth: 0,
            expandable: false,
            heading: false,
        }
    }
}
/// The reader's attachment rows, in attachment order.
fn attachment_rows() -> [&'static [LiveId]; 16] {
    [
        ids!(att0),
        ids!(att1),
        ids!(att2),
        ids!(att3),
        ids!(att4),
        ids!(att5),
        ids!(att6),
        ids!(att7),
        ids!(att8),
        ids!(att9),
        ids!(att10),
        ids!(att11),
        ids!(att12),
        ids!(att13),
        ids!(att14),
        ids!(att15),
    ]
}
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
fn pretty_folder(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "inbox" => "Inbox".into(),
        "sent messages" => "Sent".into(),
        "deleted messages" => "Trash".into(),
        _ => s.into(),
    }
}
fn display_sender(s: &str) -> String {
    let name = s.split('<').next().unwrap_or(s).trim().trim_matches('"');
    if name.is_empty() {
        s.trim_matches(['<', '>']).into()
    } else {
        name.into()
    }
}
fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}
/// A moment in local time (UTC off macOS), with its local day number.
struct Civil {
    year: i32,
    month: usize,
    day: u32,
    hour: i64,
    minute: i64,
    /// Days since 1970-01-01 in local time.
    days: i64,
}

fn civil(secs: i64) -> Civil {
    #[cfg(target_os = "macos")]
    let offset = local_offset(secs).unwrap_or(0);
    #[cfg(not(target_os = "macos"))]
    let offset = 0;
    let local = secs + offset;
    let days = local.div_euclid(86400);
    let (year, month, day) = makepad_civil_time::to_ymd(days as i32);
    let time = local.rem_euclid(86400);
    Civil { year, month: month as usize, day: day as u32, hour: time / 3600, minute: time / 60 % 60, days }
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

fn weekday(days: i64) -> &'static str {
    // 1970-01-01 was a Thursday.
    WEEKDAYS[(days + 4).rem_euclid(7) as usize]
}

fn month_name(month: usize) -> &'static str {
    MONTHS[month.clamp(1, 12) - 1]
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// The message list's date, compact as Mail's: the time today, then
/// "Yesterday", the weekday within a week, day and month this year, and
/// the full date before that.
fn list_date(secs: i64, now: i64) -> String {
    let (date, today) = (civil(secs), civil(now));
    let age = today.days - date.days;
    match age {
        0 => format!("{:02}:{:02}", date.hour, date.minute),
        1 => "Yesterday".into(),
        2..=6 => weekday(date.days).into(),
        _ if date.year == today.year && age > 0 => format!("{} {}", date.day, &month_name(date.month)[..3]),
        _ => format!("{}/{}/{}", date.day, date.month, date.year % 100),
    }
}

/// The reader's date, in full.
fn reader_date(secs: i64) -> String {
    let date = civil(secs);
    let zone = if cfg!(target_os = "macos") { "" } else { " UTC" };
    format!("{} {} {} {} at {:02}:{:02}{zone}", weekday(date.days), date.day, month_name(date.month), date.year, date.hour, date.minute)
}

/// The local zone's offset from UTC at this instant, seconds (DST applied).
#[cfg(target_os = "macos")]
fn local_offset(secs: i64) -> Option<i64> {
    use std::os::raw::{c_char, c_int, c_long};
    // Darwin's struct tm; localtime_r applies the zone and DST at this instant.
    #[repr(C)]
    struct Tm {
        sec: c_int, min: c_int, hour: c_int, day: c_int, month: c_int,
        year: c_int, weekday: c_int, yearday: c_int, dst: c_int,
        offset: c_long, zone: *const c_char,
    }
    extern "C" {
        fn localtime_r(time: *const c_long, out: *mut Tm) -> *mut Tm;
    }
    let time = c_long::try_from(secs).ok()?;
    let mut tm = std::mem::MaybeUninit::<Tm>::uninit();
    // The OS writes the complete result into caller-owned storage.
    let tm = unsafe {
        if localtime_r(&time, tm.as_mut_ptr()).is_null() { return None; }
        tm.assume_init()
    };
    Some(tm.offset as i64)
}
fn quote_query(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
fn uri_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
