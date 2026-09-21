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
    let secondary = #(crate::view::secondary_text_color(vm))
    let Text = Label{width: Fill height: Fit padding: 0 draw_text +: {color: theme.color_text wrap: Words text_style: theme.font_regular{font_size: 10.5}}}
    let Muted = Text{draw_text +: {color: secondary}}
    let Selectable = Html{width: Fill height: Fit padding: 0 selectable: true font_size: 10.5 font_color: theme.color_text paragraph_margin: Inset{top: 0 bottom: 0}}
    let Strong = Text{draw_text +: {text_style: theme.font_bold{font_size: 10.5}}}
    let Rule = SolidView{width: Fill height: 1 draw_bg.color: mix(theme.color_bg_app, theme.color_text, 0.09)}
    let Glyph = Icon{width: 16 height: 16 icon_walk: Walk{width: 16 height: 16} draw_icon +: {color: secondary}}
    let QuietButton = ButtonFlat{
        margin: 0 padding: Inset{left: 8 right: 8 top: 4 bottom: 4} height: 26
        draw_text +: {color: secondary color_hover: theme.color_text color_focus: theme.color_text text_style: theme.font_regular{font_size: 10}}
        draw_icon +: {color: secondary color_hover: theme.color_text}
        icon_walk: Walk{width: 15 height: 15}
        draw_bg +: {border_size: 0 border_radius: 6 color: #0000 color_hover: theme.color_inset_hover color_down: theme.color_bg_highlight color_focus: #0000}
    }
    let AttachmentRow = QuietButton{
        visible: false width: Fill height: 28 align: Align{x: 0.0 y: 0.5}
        draw_text +: {color: theme.color_text}
        draw_icon.svg: crate_resource("self:resources/icons/outline_attachment.svg")
    }
    let MailScrollBar = ScrollBar{bar_size: 8 bar_side_margin: 2 draw_bg +: {size: 4 border_size: 0 border_radius: 2 color: mix(theme.color_bg_app, theme.color_text, 0.25) color_hover: secondary color_drag: secondary}}
    let Hit = ButtonFlat{
        width: Fill height: Fill text: "" margin: 0 padding: 0
        draw_bg +: {pixel: fn() {return vec4(0.0, 0.0, 0.0, 0.0)}}
    }
    let FolderRow = View{
        width: Fill height: 28 flow: Overlay
        selected_bg := RoundedView{visible: false width: Fill height: Fill draw_bg +: {color: theme.color_bg_highlight border_radius: 7}}
        hit := Hit{}
        contents := View{width: Fill height: Fill flow: Right spacing: 6 align: Align{y: 0.5} padding: Inset{left: 6 right: 8}
            disclosure := View{width: 12 height: 24 flow: Overlay
                toggle_closed := QuietButton{visible: false width: 12 height: 24 padding: 0 text: "" icon_walk: Walk{width: 10 height: 10} draw_icon.svg: crate_resource("self:resources/icons/chevron.svg")}
                toggle_open := QuietButton{visible: false width: 12 height: 24 padding: 0 text: "" icon_walk: Walk{width: 10 height: 10} draw_icon.svg: crate_resource("self:resources/icons/chevron_down.svg")}
            }
            glyph := Glyph{draw_icon.svg: crate_resource("self:resources/icons/outline_inbox.svg")}
            name := Text{height: Fit max_lines: 1 text_overflow: Ellipsis}
            count := Muted{width: Fit height: Fit draw_text.text_style: theme.font_regular{font_size: 10}}
        }
    }
    let FolderHeading = View{width: Fill height: 30 padding: Inset{left: 12 top: 10 bottom: 4}
        name := Muted{draw_text.text_style: theme.font_bold{font_size: 9}}
    }
    let Message = View{
        width: Fill height: 90 flow: Overlay margin: Inset{left: 6 right: 6}
        sel := RoundedView{visible: false width: Fill height: Fill draw_bg +: {color: theme.color_bg_highlight border_radius: 8}}
        hit := Hit{}
        contents := View{width: Fill height: Fill flow: Down padding: Inset{left: 12 right: 12 top: 10 bottom: 8} spacing: 3
            View{width: Fill height: 17 flow: Right spacing: 8 align: Align{y: 0.5}
                sender := Strong{max_lines: 1 text_overflow: Ellipsis}
                date := Muted{width: 114 max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 9}}
            }
            subject := Text{height: 17 max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 10.5}}
            View{width: Fill height: 32 flow: Right spacing: 4
                preview := Muted{height: 32 max_lines: 2 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 10}}
                clip := View{visible: false width: 14 height: 14 Glyph{width: 14 height: 14 icon_walk: Walk{width: 14 height: 14} draw_icon.svg: crate_resource("self:resources/icons/outline_attachment.svg")}}
            }
        }
    }
    mod.widgets.LocalMailView = set_type_default() do mod.widgets.LocalMailViewBase{
        width: Fill height: Fill flow: Down show_bg: true draw_bg.color: theme.color_bg_app
        toolbar := View{width: Fill height: 54 flow: Right align: Align{y: 0.5} spacing: 0
            search_area := View{width: 220 height: Fill padding: Inset{left: 12 right: 12 top: 12 bottom: 12}
                search_shell := RoundedView{width: Fill height: Fill flow: Right spacing: 6 align: Align{y: 0.5} padding: Inset{left: 9 right: 5} draw_bg +: {color: theme.color_inset border_radius: 7}
                    Glyph{width: 15 height: 15 icon_walk: Walk{width: 15 height: 15} draw_icon.svg: crate_resource("self:resources/icons/search.svg")}
                    search := TextInput{width: Fill height: Fill margin: 0 padding: Inset{left: 0 top: 6 right: 0 bottom: 6} is_multiline: false empty_text: "Search mail" draw_bg +: {border_size: 0 color: #0000 color_empty: #0000 color_hover: #0000 color_focus: #0000 color_down: #0000 color_2: #0000} draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10.5}}}
                    clear_search := QuietButton{visible: false width: 22 height: 24 padding: 0 text: "×"}
                }
            }
            View{width: Fill height: Fit flow: Down spacing: 2 padding: Inset{left: 18}
                title := Strong{text: "All Mail" draw_text.text_style: theme.font_bold{font_size: 13}}
                counts := Muted{text: "All downloaded messages" draw_text.text_style: theme.font_regular{font_size: 9}}
            }
            refresh := QuietButton{text: "Refresh" draw_icon.svg: crate_resource("self:resources/icons/refresh.svg")}
            help_button := QuietButton{text: "Search tips" margin: Inset{right: 12}}
        }
        Rule{}
        help := SolidView{visible: false width: Fill height: Fit flow: Down spacing: 4 padding: 12 show_bg: true draw_bg.color: theme.color_inset
            Strong{text: "Find exactly what you’re looking for"}
            Muted{text: "Try from:alex, subject:invoice, has:attachment, after:2026-01-01, or an \"exact phrase\". Add - before a word to exclude it. Search applies within the selected mailbox."}
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
            sidebar := View{width: 220 height: Fill flow: Down
                folders := PortalList{scroll_bar: MailScrollBar{} width: Fill height: Fill margin: Inset{left: 8 right: 8} Folder := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_projects.svg")}}
                    Inbox := FolderRow{}
                    Sent := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_sent.svg")}}
                    Drafts := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_drafts.svg")}}
                    Archive := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_archive.svg")}}
                    Trash := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_trash.svg")}}
                    Attachment := FolderRow{contents +: {glyph.draw_icon.svg: crate_resource("self:resources/icons/outline_attachment.svg")}}
                    Heading := FolderHeading{}}
                sidebar_footer := View{width: Fill height: 68 flow: Down padding: Inset{left: 16 right: 14 top: 10 bottom: 8} spacing: 4
                    View{width: Fill height: Fit flow: Right spacing: 5 align: Align{y: 0.5}
                        RoundedView{width: 5 height: 5 draw_bg +: {color: theme.color_success border_radius: 2.5}}
                        account_status := Muted{text: "On this Mac" draw_text.text_style: theme.font_regular{font_size: 9}}
                    }
                    status := Muted{text: "Opening downloaded mail…" max_lines: 2 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 8.5}}
                }
            }
            Rule{width: 1 height: Fill}
            list_column := SolidView{width: 330 height: Fill flow: Down show_bg: true draw_bg.color: mix(theme.color_bg_app, theme.color_inset, 0.6)
                View{width: Fill height: 34 flow: Right align: Align{y: 0.5} padding: Inset{left: 18 right: 16}
                    list_title := Muted{text: "MESSAGES" draw_text.text_style: theme.font_bold{font_size: 8.5}}
                    Muted{width: Fit text: "Newest first" draw_text.text_style: theme.font_regular{font_size: 8.5}}
                }
                messages := PortalList{scroll_bar: MailScrollBar{} width: Fill height: Fill Message := Message{}}
                list_empty := View{visible: false width: Fill height: Fit flow: Down padding: 18 spacing: 6
                    Strong{text: "No messages found"}
                    Muted{text: "Try a different search or choose another mailbox."}
                }
                more := QuietButton{visible: false width: Fill height: 28 text: "Load more messages"}
            }
            Rule{width: 1 height: Fill}
            reader_column := SolidView{width: Fill height: Fill flow: Down show_bg: true draw_bg.color: theme.color_inset
                reader_toolbar := View{width: Fill height: 34 flow: Right spacing: 3 align: Align{y: 0.5} padding: Inset{left: 16 right: 16}
                    mailbox := Muted{text: "No message selected" max_lines: 1 text_overflow: Ellipsis draw_text.text_style: theme.font_regular{font_size: 9}}
                    reply := QuietButton{visible: false text: "Reply" draw_icon.svg: crate_resource("self:resources/icons/reply.svg")}
                    open_mail := QuietButton{visible: false text: "Open in Mail" draw_icon.svg: crate_resource("self:resources/icons/forward.svg")}
                }
                reader_placeholder := View{width: Fill height: Fill flow: Down align: Align{x: 0.5 y: 0.45} spacing: 10 padding: 28
                    Icon{width: 44 height: 44 icon_walk: Walk{width: 44 height: 44} draw_icon +: {svg: crate_resource("self:resources/icons/outline_inbox.svg") color: secondary}}
                    Strong{width: Fit text: "No message selected" draw_text.text_style: theme.font_bold{font_size: 14}}
                    Muted{width: Fit text: "Select a message to start reading."}
                }
                reader := ScrollYView{scroll_bars +: {scroll_bar_y: MailScrollBar{}} visible: false width: Fill height: Fill flow: Down
                    reader_content := View{width: Fill height: Fit flow: Down spacing: 14 padding: Inset{left: 24 right: 24 top: 18 bottom: 32}
                        subject := Selectable{font_size: 18}
                        View{width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                            avatar := RoundedView{width: 34 height: 34 align: Align{x: 0.5 y: 0.5} draw_bg +: {color: theme.color_bg_highlight border_radius: 17}
                                initials := Strong{width: Fit draw_text +: {color: theme.color_focus text_style: theme.font_bold{font_size: 12.5}}}
                            }
                            View{width: Fill height: Fit flow: Down spacing: 4
                                from := Selectable{}
                                to := Selectable{font_size: 9.5 font_color: secondary}
                            }
                        }
                        date := Selectable{font_size: 9 font_color: secondary}
                        Rule{}
                        body := Html{selectable: true width: Fill height: Fit padding: 0 font_size: 11.5 font_color: theme.color_text paragraph_margin: Inset{top: 0.3 bottom: 0.5} draw_text.color: theme.color_text
                            a := HtmlLink{color: theme.color_focus hover_color: theme.color_text pressed_color: theme.color_focus}
                        }
                        attachments := View{visible: false width: Fill height: Fit flow: Down spacing: 8
                            Rule{}
                            Muted{text: "ATTACHMENTS" draw_text.text_style: theme.font_bold{font_size: 8.5}}
                            View{width: Fill height: Fit flow: Down spacing: 2 padding: 6 show_bg: true draw_bg.color: theme.color_bg_app
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
    limit: usize,
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
    selected_message: Option<MessageSummary>,
    #[rust]
    help_open: bool,
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
        self.limit = 200;
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
            limit: self.limit,
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
                    let folder_change = !Arc::ptr_eq(&self.mailbox_catalog, &page.mailboxes);
                    self.page = page;
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
                    self.view
                        .widget(cx, ids!(more))
                        .set_visible(cx, self.page.total > self.page.hits.len());
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
    fn select(&mut self, cx: &mut Cx, message: MessageSummary) {
        self.selected = Some(message.id);
        for path in [ids!(subject), ids!(from), ids!(to), ids!(date), ids!(body)] {
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
        self.view
            .html(cx, ids!(date))
            .set_text(cx, &display_date(message.date));
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
    fn rebuild_folders(&mut self) {
        let mut rows = vec![
            FolderItem::heading("Favorites"),
            FolderItem::smart("", "All Mail", "", live_id!(Inbox), self.page.indexed),
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
            self.limit = 200;
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
                self.limit = 200;
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
        if self.view.button(cx, ids!(more)).clicked(actions) {
            self.limit += 200;
            self.queue_search();
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
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(message) = self.page.hits.get(index) else {
                        continue;
                    };
                    let item = list.item(cx, index, live_id!(Message));
                    item.label(cx, ids!(sender))
                        .set_text(cx, &display_sender(&message.from));
                    item.label(cx, ids!(subject)).set_text(cx, &message.subject);
                    item.label(cx, ids!(preview)).set_text(cx, &message.preview);
                    item.label(cx, ids!(date)).set_text(
                        cx,
                        &display_date(message.date),
                    );
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
fn display_date(secs: i64) -> String {
    #[cfg(target_os = "macos")]
    if let Some(date) = local_date(secs) { return date; }
    let (year, month, day) = makepad_civil_time::to_ymd(secs.div_euclid(86400) as i32);
    let time = secs.rem_euclid(86400);
    format!("{:02}:{:02} {day}/{month}/{year} UTC", time / 3600, time / 60 % 60)
}

#[cfg(target_os = "macos")]
fn local_date(secs: i64) -> Option<String> {
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
    Some(format!("{:02}:{:02} {}/{}/{}", tm.hour, tm.min, tm.day, tm.month + 1, tm.year + 1900))
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
