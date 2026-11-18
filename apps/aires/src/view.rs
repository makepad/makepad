use crate::source::{Answer, AnswerResult, Cleared, Snapshot};
use makepad_civil_time::{local_or_utc, MONTH_ABBR, WEEKDAY_ABBR};
use makepad_devhub::ask::{kept_words, untaken, Msg, Question, State};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Where a question's card stands: the person's turn, main's turn, or done.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// The inbox's own head
    Top,
    Yours,
    Main,
    History,
}

impl Section {
    fn of(state: State) -> Self {
        match state {
            State::Open => Self::Yours,
            State::Waiting => Self::Main,
            State::Answered | State::Closed => Self::History,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Top => "",
            Self::Yours => "YOUR TURN",
            Self::Main => "WITH MAIN",
            Self::History => "HISTORY",
        }
    }

    /// The card's stripe: the person's turn in the primary role, main's in the tertiary, none
    /// for history.
    fn tone(self) -> f64 {
        match self {
            Self::Yours => 1.0,
            Self::Main => 2.0,
            Self::Top | Self::History => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub section: Section,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub enum Kind {
    /// The list's head: the counts, what the last clear did, and the connection's state when it
    /// is not fine
    Header { open: usize, waiting: usize, history: usize, note: String, status: String },
    /// A section's line; `clear`: the history's, with its Clear link
    Section { count: usize, clear: bool },
    /// The history's clear, asking first: how many go to the archive, and what stays and why
    Confirm { cleared: usize, open: usize, untaken: usize, pending: bool },
    /// A card's top: `toggle` is `Some(shown)` for a card that folds (not the person's turn)
    Head { title: String, from: String, last_ms: u64, state: State, refused: bool, new: bool, toggle: Option<bool> },
    /// The last answer's error or refusal, in the card under its head
    Notice(String),
    Field { label: &'static str, text: String },
    /// The context's switch; its text follows as a `ContextBody` while shown
    Context { shown: bool },
    ContextBody(String),
    /// A message of the card's thread; `brief`: the one line a folded card shows
    Msg { message: Msg, option: String, brief: bool },
    Option { number: u32, text: String, recommended: bool },
    Decide,
    Input,
    /// A card's bottom edge
    End,
    Gap,
    Empty,
}

/// A row's identity across snapshots. The section is part of it: a question that changes
/// section leaves its old card and enters a new one.
type RowKey = (String, Section, &'static str, u32);

const SECTIONS: [Section; 3] = [Section::Yours, Section::Main, Section::History];

impl Row {
    fn key(&self) -> RowKey {
        let (kind, number) = match &self.kind {
            Kind::Header { .. } => ("header", 0),
            Kind::Section { .. } => ("section", 0),
            Kind::Confirm { .. } => ("confirm", 0),
            Kind::Head { .. } => ("head", 0),
            Kind::Notice(_) => ("notice", 0),
            Kind::Field { label, .. } => (*label, 0),
            Kind::Context { .. } => ("context", 0),
            Kind::ContextBody(_) => ("context body", 0),
            Kind::Msg { message, .. } => ("message", message.seq),
            Kind::Option { number, .. } => ("option", *number),
            Kind::Decide => ("decide", 0),
            Kind::Input => ("input", 0),
            Kind::End => ("end", 0),
            Kind::Gap => ("gap", 0),
            Kind::Empty => ("empty", 0),
        };
        (self.id.clone(), self.section, kind, number)
    }

    /// A row that is part of a card (drawn on the card's surface)
    fn on_card(&self) -> bool {
        !matches!(self.kind, Kind::Header { .. } | Kind::Section { .. } | Kind::Confirm { .. } | Kind::Gap | Kind::Empty)
    }
}

fn context_key(id: &str) -> String {
    format!("context:{id}")
}

fn message(q: &Question, m: &Msg, brief: bool) -> Kind {
    let option = m.choice.and_then(|n| n.checked_sub(1)).and_then(|n| q.options.get(n as usize)).cloned().unwrap_or_default();
    Kind::Msg { message: m.clone(), option, brief }
}

/// The last thing that happened to a question
fn last_ms(q: &Question) -> u64 {
    q.thread.iter().map(|m| m.at_ms).max().unwrap_or(0).max(q.created_ms)
}

/// The line in a card under its head: the last answer's error, else its refused answers.
fn notice(q: &Question, errors: &HashMap<String, String>) -> Option<String> {
    if let Some(error) = errors.get(&q.id) {
        return Some(error.clone());
    }
    let last = q.refused.last()?;
    let why = if last.why.is_empty() { &last.chain } else { &last.why };
    Some(match q.refused.len() {
        1 => format!("An answer was refused: {why}"),
        n => format!("{n} answers were refused, the last: {why}"),
    })
}

/// Where the person's clear of the history stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Clearing {
    #[default]
    Idle,
    /// Its confirm is shown
    Confirm,
    /// Sent, its answer not back yet
    Pending,
}

/// The clear's part of the list: where it stands, and what the last one did.
#[derive(Clone, Debug, Default)]
pub struct ClearView {
    pub clearing: Clearing,
    pub note: Option<String>,
}

pub fn rows(questions: &[Question], expanded: &HashSet<String>, errors: &HashMap<String, String>, unseen: &HashSet<String>, status: Option<&str>, clear: &ClearView) -> Vec<Row> {
    let count = |section| questions.iter().filter(|q| Section::of(q.state) == section).count();
    let header = Kind::Header {
        open: count(Section::Yours), waiting: count(Section::Main), history: count(Section::History),
        note: clear.note.clone().unwrap_or_default(), status: status.unwrap_or("").into(),
    };
    let mut out = vec![Row { id: String::new(), section: Section::Top, kind: header }];
    for section in SECTIONS {
        let mut cards: Vec<&Question> = questions.iter().filter(|q| Section::of(q.state) == section).collect();
        if cards.is_empty() { continue; }
        // Main's turn keeps the master's order (the newest on top); the person's turn and the
        // history show the latest change first: a question main reopened with a reply comes to
        // the top, not under the ones filed after it.
        if section != Section::Main { cards.sort_by_key(|q| std::cmp::Reverse(last_ms(q))); }
        let history = section == Section::History;
        out.push(Row { id: String::new(), section, kind: Kind::Section { count: cards.len(), clear: history && clear.clearing == Clearing::Idle } });
        // (the master clears what main has taken every message of; open questions and untaken
        // answers stay, and the confirm says so)
        if history && clear.clearing != Clearing::Idle {
            let untaken = cards.iter().filter(|q| untaken(q)).count();
            out.push(Row { id: String::new(), section, kind: Kind::Confirm {
                cleared: cards.len() - untaken, open: count(Section::Yours) + count(Section::Main), untaken, pending: clear.clearing == Clearing::Pending,
            } });
        }
        for q in cards {
            let mut push = |kind| out.push(Row { id: q.id.clone(), section, kind });
            // the person's turn shows the whole card; the others fold to their last message
            let full = section == Section::Yours || expanded.contains(&q.id);
            push(Kind::Head {
                title: q.title.clone(), from: q.from.clone(), last_ms: last_ms(q), state: q.state,
                refused: !q.refused.is_empty(), new: unseen.contains(&q.id), toggle: (section != Section::Yours).then_some(full),
            });
            if let Some(text) = notice(q, errors) { push(Kind::Notice(text)); }
            if full {
                for (label, text) in [("Why now", &q.why), ("Blocks", &q.blocked), ("Decided", &q.decided)] {
                    if !text.is_empty() { push(Kind::Field { label, text: text.clone() }); }
                }
                // (a recommended option shows as the marked choice; one that is no option is said)
                if !q.recommend.is_empty() && !q.options.contains(&q.recommend) {
                    push(Kind::Field { label: "Suggests", text: q.recommend.clone() });
                }
                if !q.context.is_empty() && q.state != State::Closed {
                    let shown = expanded.contains(&context_key(&q.id));
                    push(Kind::Context { shown });
                    if shown { push(Kind::ContextBody(q.context.clone())); }
                }
                // the conversation so far, then the choice
                for m in &q.thread { push(message(q, m, false)); }
                if q.state != State::Closed {
                    for (i, text) in q.options.iter().enumerate() {
                        push(Kind::Option { number: (i + 1) as u32, text: text.clone(), recommended: *text == q.recommend });
                    }
                    push(Kind::Decide);
                    push(Kind::Input);
                }
            } else {
                let last = match q.state {
                    State::Answered => q.thread.iter().rev().find(|m| m.by == "user" && m.kind == "answer"),
                    _ => q.thread.last(),
                };
                if let Some(m) = last { push(message(q, m, true)); }
            }
            push(Kind::End);
            push(Kind::Gap);
        }
    }
    if questions.is_empty() && status.is_none() { out.push(Row { id: String::new(), section: Section::Top, kind: Kind::Empty }); }
    out
}

#[derive(Default)]
pub struct Model {
    pub questions: Vec<Question>,
    pub rows: Vec<Row>,
    pub drafts: HashMap<String, String>,
    pub errors: HashMap<String, String>,
    /// The answers on their way to the master, by question
    pub pending: HashMap<String, Answer>,
    expanded: HashSet<String>,
    /// The questions that became the person's turn since they last looked (the dropdown closed)
    pub unseen: HashSet<String>,
    /// The person's turns at the last snapshot
    open: HashSet<String>,
    /// A snapshot of the questions has arrived
    pub loaded: bool,
    pub status: Option<String>,
    pub clear: ClearView,
    revision: u64,
}

impl Model {
    pub fn rebuild(&mut self) {
        self.revision += 1;
        self.rows = rows(&self.questions, &self.expanded, &self.errors, &self.unseen, self.status.as_deref(), &self.clear);
    }

    /// The master's answer to the person's clear: the header says what went and what stayed.
    pub fn cleared(&mut self, result: Result<Cleared, String>) {
        self.clear.clearing = Clearing::Idle;
        self.clear.note = Some(match result {
            Ok(c) => format!("Cleared {} to the archive{}", c.cleared, kept_words(c.open, c.untaken)),
            Err(error) => format!("Not cleared: {error}"),
        });
        self.rebuild();
    }

    pub fn update(&mut self, snapshot: Option<Snapshot>, results: Vec<AnswerResult>) {
        if let Some(snapshot) = snapshot {
            if let Some(questions) = snapshot.questions {
                let open: HashSet<String> = questions.iter().filter(|q| q.state == State::Open).map(|q| q.id.clone()).collect();
                // (the turns at the first snapshot were there before the app, so nothing is new)
                if self.loaded { self.unseen.extend(open.difference(&self.open).cloned()); }
                self.unseen.retain(|id| open.contains(id));
                self.open = open;
                self.questions = questions;
                self.loaded = true;
            }
            self.status = snapshot.error;
        }
        for AnswerResult { answer, result } in results {
            self.pending.remove(&answer.id);
            match result {
                Ok(()) => {
                    // An edit made while the request was in flight belongs to the next answer.
                    if self.drafts.get(&answer.id) == Some(&answer.text) { self.drafts.remove(&answer.id); }
                }
                Err(error) => { self.errors.insert(answer.id, error); }
            }
        }
        self.rebuild();
    }

    pub fn waiting(&self) -> usize {
        self.questions.iter().filter(|q| q.state == State::Open).count()
    }

    /// The person has looked: nothing is new any more, and a clear's note or confirm goes.
    pub fn mark_seen(&mut self) {
        if !self.unseen.is_empty() || self.clear.note.is_some() || self.clear.clearing == Clearing::Confirm {
            self.unseen.clear();
            self.clear.note = None;
            if self.clear.clearing == Clearing::Confirm { self.clear.clearing = Clearing::Idle; }
            self.rebuild();
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum InboxAction {
    Answer(Answer),
    /// The person confirmed the clear of the history
    Clear,
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Text = Label{
        width: Fill height: Fit flow: Right{wrap: true}
        padding: Inset{}
        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 9.5}}
    }
    let Meta = Label{
        width: Fit height: Fit
        padding: Inset{}
        draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 8.5}}
    }
    let Chip = RoundedView{
        width: Fit height: Fit
        padding: Inset{left: 6 right: 6 top: 2 bottom: 3}
        draw_bg +: {border_radius: 4.0}
    }
    let ChipText = Label{padding: Inset{} draw_text +: {text_style: theme.font_bold{font_size: 7.5}}}
    let Link = ButtonFlatter{
        width: Fit height: Fit
        margin: Inset{}
        padding: Inset{left: 6 right: 6 top: 3 bottom: 3}
        draw_text +: {
            color: theme.color_primary color_hover: theme.color_primary color_down: theme.color_primary color_focus: theme.color_primary
            text_style: theme.font_regular{font_size: 8.5}
        }
    }
    // One row of a card. A card is drawn by its rows: its head rounds the upper corners, its end
    // the lower ones, and an open edge runs past the row, so no seam shows where two rows meet.
    // A card on someone's turn has a stripe down its left edge (tone 1: the person's, 2: main's);
    // a card that just arrived glows for a moment.
    let Piece = SolidView{
        width: Fill height: Fit flow: Down
        margin: Inset{left: 12 right: 12}
        padding: Inset{left: 18 right: 16 top: 3 bottom: 3}
        draw_bg +: {
            color: theme.color_surface_bright
            tone: instance(0.0)
            glow: instance(0.0)
            radius_top: uniform(0.0)
            radius_bottom: uniform(0.0)
            color_yours: uniform(theme.color_primary)
            color_main: uniform(theme.color_tertiary)
            color_glow: uniform(theme.color_primary)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let top = if self.radius_top > 0.0 0.0 else -4.0
                let bottom = if self.radius_bottom > 0.0 self.rect_size.y else self.rect_size.y + 4.0
                sdf.box_all(0.0, top, self.rect_size.x, bottom - top, self.radius_top, self.radius_top, self.radius_bottom, self.radius_bottom)
                sdf.fill_keep(mix(self.color, self.color_glow, self.glow * 0.16))
                if self.tone > 0.5 {
                    let stripe = if self.tone > 1.5 self.color_main else self.color_yours
                    sdf.rect(0.0, top, 3.0, bottom - top)
                    sdf.intersect()
                    sdf.fill(stripe)
                }
                return sdf.result
            }
        }
    }
    // An option: the whole card's width, its text wrapping, the recommended one filled.
    let Choice = ButtonFlat{
        width: Fill height: Fit
        margin: Inset{}
        padding: Inset{left: 12 right: 12 top: 9 bottom: 9}
        flow: Right{wrap: true}
        align: Align{x: 0.0 y: 0.5}
        label_walk: Walk{width: Fill height: Fit}
        label_align: Align{x: 0.0 y: 0.5}
        draw_text +: {
            color: theme.color_on_surface color_hover: theme.color_on_surface color_down: theme.color_on_surface color_focus: theme.color_on_surface
            text_style: theme.font_regular{font_size: 10}
        }
        draw_bg +: {
            border_size: 1.0
            border_radius: 8.0
            // a step from the card toward the ink: lighter on a dark card, darker on a light one
            color: mix(theme.color_surface_bright, theme.color_text, 0.07)
            color_default: mix(theme.color_surface_bright, theme.color_text, 0.07)
            color_hover: mix(theme.color_surface_bright, theme.color_text, 0.07)
            color_down: mix(theme.color_surface_bright, theme.color_text, 0.07)
            color_focus: mix(theme.color_surface_bright, theme.color_text, 0.07)
            border_color: theme.color_outline_variant
            border_color_hover: theme.color_outline
            border_color_down: theme.color_primary
            border_color_focus: theme.color_primary
            border_color_disabled: theme.color_outline_variant
            layer_color: theme.color_primary
        }
    }
    let Recommended = Choice{
        description: "Recommended"
        draw_text +: {
            color: theme.color_on_primary color_hover: theme.color_on_primary color_down: theme.color_on_primary color_focus: theme.color_on_primary
            text_style: theme.font_bold{font_size: 10}
        }
        draw_description +: {
            color: mix(theme.color_on_primary, theme.color_primary, 0.3)
            text_style: theme.font_regular{font_size: 7.5}
        }
        draw_bg +: {
            color: theme.color_primary
            color_default: theme.color_primary
            color_hover: theme.color_primary
            color_down: theme.color_primary
            color_focus: theme.color_primary
            border_color: theme.color_primary
            border_color_hover: theme.color_primary
            border_color_down: theme.color_primary
            border_color_focus: theme.color_on_primary
            layer_color: theme.color_on_primary
        }
    }
    let Delegate = Choice{
        text: "Let main decide"
        draw_text +: {
            color: theme.color_text_meta color_hover: theme.color_text color_down: theme.color_text color_focus: theme.color_text
            text_style: theme.font_regular{font_size: 9}
        }
        draw_bg +: {
            border_dash: 4.0
            color: theme.color_u_hidden
            color_default: theme.color_u_hidden
            color_hover: theme.color_u_hidden
            color_down: theme.color_u_hidden
            color_focus: theme.color_u_hidden
            color_disabled: theme.color_u_hidden
        }
    }
    mod.widgets.InboxBase = #(Inbox::register_widget(vm))
    mod.widgets.Inbox = set_type_default() do mod.widgets.InboxBase{
        width: Fill height: Fill
        list := PortalList{
            width: Fill height: Fill
            scroll_bar: ScrollBar{}
            scan: [@Header, @Section, @Confirm, @Head, @Notice, @Field, @Context, @ContextBody, @Msg, @Option, @Decide, @Input, @End, @Gap, @Empty]
            Header := View{
                width: Fill height: Fit flow: Down spacing: 3
                padding: Inset{left: 22 right: 18 top: 16 bottom: 2}
                View{
                    width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                    Label{text: "Questions" padding: Inset{} draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 13}}}
                    View{width: Fill height: Fit}
                    open := Chip{
                        padding: Inset{left: 9 right: 9 top: 3 bottom: 4}
                        draw_bg +: {color: theme.color_primary border_radius: 9.0}
                        open_count := ChipText{draw_text +: {color: theme.color_on_primary text_style: theme.font_bold{font_size: 8.5}}}
                    }
                    clear := Chip{
                        padding: Inset{left: 9 right: 9 top: 3 bottom: 4}
                        draw_bg +: {color: theme.color_success_container border_radius: 9.0}
                        ChipText{text: "All clear" draw_text +: {color: theme.color_on_success_container text_style: theme.font_bold{font_size: 8.5}}}
                    }
                }
                summary := Meta{}
                note := Meta{}
                status := View{
                    width: Fill height: Fit flow: Right spacing: 7 align: Align{y: 0.5}
                    padding: Inset{top: 6}
                    CircleView{width: 7 height: 7 draw_bg +: {color: theme.color_error}}
                    status_text := Text{draw_text +: {color: theme.color_error text_style: theme.font_regular{font_size: 8.5}}}
                }
            }
            Section := View{
                width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                padding: Inset{left: 24 right: 20 top: 12 bottom: 8}
                text := Label{draw_text +: {color: theme.color_text_meta text_style: theme.font_bold{font_size: 7.5}}}
                count := Label{draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 7.5}}}
                View{width: Fill height: Fit}
                clear := Link{text: "Clear" draw_text +: {
                    color: theme.color_text_meta color_hover: theme.color_text color_down: theme.color_text color_focus: theme.color_text
                    text_style: theme.font_regular{font_size: 8}
                }}
            }
            Confirm := View{
                width: Fill height: Fit
                padding: Inset{left: 12 right: 12 bottom: 10}
                RoundedView{
                    width: Fill height: Fit flow: Down spacing: 8
                    padding: Inset{left: 12 right: 10 top: 9 bottom: 9}
                    draw_bg +: {color: mix(theme.color_surface_bright, theme.color_text, 0.05) border_radius: 8.0}
                    text := Text{draw_text +: {color: theme.color_on_surface_variant text_style: theme.font_regular{font_size: 9}}}
                    View{
                        width: Fill height: Fit flow: Right spacing: 6 align: Align{x: 1.0 y: 0.5}
                        cancel := ButtonOutline{width: Fit text: "Cancel"}
                        confirm := ButtonPrimary{width: Fit text: "Clear"}
                    }
                }
            }
            Head := Piece{
                padding: Inset{left: 18 right: 10 top: 12 bottom: 5}
                spacing: 7
                draw_bg +: {radius_top: 12.0}
                View{
                    width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                    chip_answered := Chip{draw_bg +: {color: theme.color_success_container} ChipText{text: "Answered" draw_text +: {color: theme.color_on_success_container}}}
                    chip_closed := Chip{draw_bg +: {color: theme.color_surface_container_highest} ChipText{text: "Closed" draw_text +: {color: theme.color_on_surface_variant}}}
                    chip_refused := Chip{draw_bg +: {color: theme.color_error_container} ChipText{text: "Refused" draw_text +: {color: theme.color_on_error_container}}}
                    chip_new := Chip{draw_bg +: {color: theme.color_primary} ChipText{text: "New" draw_text +: {color: theme.color_on_primary}}}
                    meta := Meta{}
                    View{width: Fill height: Fit}
                    toggle := Link{text: "Show"}
                }
                title := Text{
                    padding: Inset{right: 6}
                    draw_text +: {text_style: theme.font_bold{font_size: 11}}
                }
            }
            Notice := Piece{
                padding: Inset{left: 18 right: 16 top: 4 bottom: 6}
                RoundedView{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 10 right: 10 top: 7 bottom: 8}
                    draw_bg +: {color: mix(theme.color_error_container, theme.color_surface_bright, 0.55) border_radius: 6.0}
                    text := Text{draw_text +: {color: theme.color_on_error_container text_style: theme.font_regular{font_size: 8.5}}}
                }
            }
            Field := Piece{
                flow: Right spacing: 10
                label := Label{
                    width: 58 height: Fit
                    padding: Inset{top: 2}
                    margin: Inset{}
                    draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 8}}
                }
                text := Text{}
            }
            Context := Piece{
                padding: Inset{left: 12 right: 16 top: 2 bottom: 2}
                toggle := Link{text: "Show context"}
            }
            ContextBody := Piece{
                padding: Inset{left: 18 right: 16 top: 2 bottom: 6}
                RoundedView{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 10 right: 10 top: 8 bottom: 9}
                    draw_bg +: {color: mix(theme.color_surface_bright, theme.color_text, 0.05) border_radius: 6.0}
                    text := Text{draw_text +: {color: theme.color_on_surface_variant text_style: theme.font_regular{font_size: 9}}}
                }
            }
            Msg := Piece{
                padding: Inset{left: 18 right: 16 top: 4 bottom: 4}
                spacing: 3
                who := Meta{draw_text +: {text_style: theme.font_bold{font_size: 8}}}
                text := Text{draw_text +: {color: theme.color_on_surface_variant text_style: theme.font_regular{font_size: 9}}}
            }
            Option := Piece{
                padding: Inset{left: 18 right: 16 top: 3 bottom: 3}
                choose := Choice{}
                pick := Recommended{}
            }
            Decide := Piece{
                padding: Inset{left: 18 right: 16 top: 3 bottom: 3}
                decide := Delegate{}
            }
            Input := Piece{
                flow: Right spacing: 6 align: Align{y: 0.5}
                padding: Inset{left: 18 right: 16 top: 8 bottom: 2}
                draft := TextInput{
                    width: Fill height: Fit
                    is_multiline: true
                    flow: Right{wrap: true}
                    wrap_empty_text: true
                    empty_text: "Answer in your words, or ask"
                }
                ask := ButtonOutline{width: Fit text: "Ask"}
                answer := ButtonPrimary{width: Fit text: "Answer"}
            }
            End := Piece{height: 14 padding: Inset{} draw_bg +: {radius_bottom: 12.0}}
            Gap := View{width: Fill height: 10}
            Empty := View{
                width: Fill height: Fit flow: Down spacing: 4 align: Align{x: 0.5}
                padding: Inset{top: 48 bottom: 48}
                Label{text: "All clear" draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 12}}}
                Meta{text: "No questions wait for you."}
            }
        }
    }
}

/// How a block of rows opens (to 1) or closes (to 0): the part of its height shown.
#[derive(Clone, Copy, Debug)]
struct Motion {
    start: f64,
    from: f64,
    to: f64,
}

/// Seconds a block takes to open, and to close.
const OPEN_S: f64 = 0.28;
const CLOSE_S: f64 = 0.22;
/// Seconds a new card's glow takes to fade, after it has opened.
const GLOW_S: f64 = 1.6;

impl Motion {
    fn duration(&self) -> f64 {
        (if self.to > self.from { OPEN_S } else { CLOSE_S }) * (self.to - self.from).abs().max(0.25)
    }

    fn done(&self, now: f64) -> bool {
        now >= self.start + self.duration()
    }

    fn at(&self, now: f64) -> f64 {
        let t = ((now - self.start) / self.duration()).clamp(0.0, 1.0);
        // ease out: quick to start, soft to land
        self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3))
    }
}

/// A row the list shows: one of the model's (live), or one on its way out (a ghost) that
/// keeps what it showed until its block has closed.
struct Slot {
    key: RowKey,
    row: Row,
    live: bool,
    /// The block it opens or closes with (0: settled)
    block: u64,
    /// Its height at its last draw, unclipped
    height: f64,
}

/// What a row of the old list does when the model's rows change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The old slot shows the new row
    Keep(usize, usize),
    /// The old slot has no row any more
    Leave(usize),
    /// A new row
    Enter(usize),
}

/// The longest run of increasing values (by position in `seq`): the slots that stay in order.
fn longest_increasing(seq: &[usize]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![None; seq.len()];
    for i in 0..seq.len() {
        let at = tails.partition_point(|&t| seq[t] < seq[i]);
        if at > 0 { prev[i] = Some(tails[at - 1]); }
        if at == tails.len() { tails.push(i) } else { tails[at] = i }
    }
    let mut keep = vec![false; seq.len()];
    let mut at = tails.last().copied();
    while let Some(i) = at {
        keep[i] = true;
        at = prev[i];
    }
    keep
}

/// The steps from the old slots (key, live) to the new rows: the slots that keep their order
/// and key stay, the rest leave where they were and the new rows enter where they go. One old
/// slot per key can stay: the live one, else a ghost (which comes back).
fn merge(old: &[(&RowKey, bool)], new: &[RowKey]) -> Vec<Step> {
    let at: HashMap<&RowKey, usize> = new.iter().enumerate().map(|(i, key)| (key, i)).collect();
    let mut claimed: HashMap<&RowKey, usize> = HashMap::new();
    for (i, &(key, live)) in old.iter().enumerate() {
        if !at.contains_key(key) { continue; }
        match claimed.get(key) {
            Some(&j) if old[j].1 || !live => {}
            _ => { claimed.insert(key, i); }
        }
    }
    let candidates: Vec<usize> = (0..old.len()).filter(|&i| claimed.get(old[i].0) == Some(&i)).collect();
    let seq: Vec<usize> = candidates.iter().map(|&i| at[old[i].0]).collect();
    let keep = longest_increasing(&seq);
    let mut target = vec![None; old.len()];
    for (n, &i) in candidates.iter().enumerate() {
        if keep[n] { target[i] = Some(seq[n]); }
    }
    let mut steps = Vec::with_capacity(old.len() + new.len());
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        match target.get(i).copied() {
            Some(None) => { steps.push(Step::Leave(i)); i += 1; }
            Some(Some(t)) if t == j => { steps.push(Step::Keep(i, j)); i += 1; j += 1; }
            _ => { steps.push(Step::Enter(j)); j += 1; }
        }
    }
    steps
}

#[derive(Script, ScriptHook, Widget)]
pub struct Inbox {
    #[deref]
    view: View,
    #[rust]
    pub model: Rc<RefCell<Model>>,
    /// The live rows by the widget that showed them, for routing presses
    #[rust]
    drawn: HashMap<WidgetUid, Row>,
    #[rust]
    slots: Vec<Slot>,
    #[rust]
    blocks: HashMap<u64, Motion>,
    #[rust]
    next_block: u64,
    /// The cards that just arrived, by question: when they began to open
    #[rust]
    glows: HashMap<String, f64>,
    /// The card rows' widgets that have a stripe or glow, and which
    #[rust]
    applied: HashMap<WidgetUid, (f64, f64)>,
    /// The model had questions at the last sync: changes from here on move
    #[rust]
    loaded: bool,
    #[rust]
    revision: u64,
    #[rust]
    next_frame: NextFrame,
    /// Ages and times in the cards move on
    #[rust]
    tick: Timer,
}

impl Inbox {
    fn reveal(&self, block: u64, now: f64) -> f64 {
        self.blocks.get(&block).map_or(1.0, |m| m.at(now))
    }

    /// The block of the current run of like steps, or a new one.
    fn block_for(&mut self, run: &mut Option<(u8, u64)>, kind: u8, from: f64, to: f64, now: f64) -> u64 {
        if let Some((k, block)) = *run {
            if k == kind { return block; }
        }
        self.next_block += 1;
        let block = self.next_block;
        self.blocks.insert(block, Motion { start: now, from, to });
        *run = Some((kind, block));
        block
    }

    fn sync_rows(&mut self, cx: &mut Cx) {
        let model_ref = self.model.clone();
        let model = model_ref.borrow();
        if self.revision == model.revision { return; }
        let list_ref = self.view.portal_list(cx, ids!(list));
        let Some(mut list) = list_ref.borrow_mut() else { return };
        self.revision = model.revision;
        let now = cx.seconds_since_app_start();
        // The first questions are there at once; what changes after that moves.
        let animate = self.loaded && model.loaded;
        self.loaded = model.loaded;
        let at_top = list.first_id() == 0 && list.first_scroll() >= -0.5;
        list.set_item_range(cx, 0, self.slots.len());
        let keys: Vec<RowKey> = model.rows.iter().map(Row::key).collect();
        let steps = merge(&self.slots.iter().map(|s| (&s.key, s.live)).collect::<Vec<_>>(), &keys);
        let mut old: Vec<Option<Slot>> = std::mem::take(&mut self.slots).into_iter().map(Some).collect();
        let mut slots = Vec::with_capacity(keys.len());
        // A run of like steps opens or closes as one block (0 enter, 1 leave, 2 come back).
        let mut run = None;
        // Splice the list's items along, so the kept rows keep their widgets (an input its
        // focus and cursor); removed controls lose their captures through PortalList's own
        // splice handling.
        for step in steps {
            let pos = slots.len();
            match step {
                Step::Keep(i, j) => {
                    let Some(mut slot) = old[i].take() else { continue };
                    slot.row = model.rows[j].clone();
                    if slot.live {
                        run = None;
                    } else {
                        slot.live = true;
                        let from = self.reveal(slot.block, now);
                        slot.block = self.block_for(&mut run, 2, from, 1.0, now);
                    }
                    slots.push(slot);
                }
                Step::Leave(i) => {
                    let Some(mut slot) = old[i].take() else { continue };
                    if !animate {
                        list.splice_items(cx, pos, 1, 0);
                        continue;
                    }
                    if slot.live {
                        slot.live = false;
                        let from = self.reveal(slot.block, now);
                        slot.block = self.block_for(&mut run, 1, from, 0.0, now);
                    }
                    slots.push(slot);
                }
                Step::Enter(j) => {
                    list.splice_items(cx, pos, 0, 1);
                    let row = model.rows[j].clone();
                    let block = if animate { self.block_for(&mut run, 0, 0.0, 1.0, now) } else { 0 };
                    // a card that arrives glows: a new question, or one that changed its turn
                    if animate && matches!(row.kind, Kind::Head { .. }) { self.glows.insert(row.id.clone(), now); }
                    slots.push(Slot { key: keys[j].clone(), row, live: true, block, height: 0.0 });
                }
            }
        }
        self.slots = slots;
        if at_top {
            list.set_first_id_and_scroll(0, 0.0);
        } else if list.first_id() >= self.slots.len() {
            list.set_first_id_and_scroll(self.slots.len().saturating_sub(1), 0.0);
        }
    }

    /// Blocks that finished: an opened one settles, a closed one's ghosts go.
    fn settle(&mut self, cx: &mut Cx, now: f64) {
        self.glows.retain(|_, start| now < *start + OPEN_S + GLOW_S + 0.5);
        let done: HashSet<u64> = self.blocks.iter().filter(|(_, m)| m.done(now)).map(|(&b, _)| b).collect();
        if done.is_empty() { return; }
        let list_ref = self.view.portal_list(cx, ids!(list));
        let Some(mut list) = list_ref.borrow_mut() else { return };
        list.set_item_range(cx, 0, self.slots.len());
        for i in (0..self.slots.len()).rev() {
            if !done.contains(&self.slots[i].block) { continue; }
            if self.slots[i].live {
                self.slots[i].block = 0;
            } else {
                self.slots.remove(i);
                list.splice_items(cx, i, 1, 0);
            }
        }
        for block in done { self.blocks.remove(&block); }
    }

    fn glow(&self, id: &str, now: f64) -> f64 {
        self.glows.get(id).map_or(0.0, |start| (1.0 - (now - start - OPEN_S) / GLOW_S).clamp(0.0, 1.0))
    }
}

/// How long something has waited: `3m`, `2h`, `4d`.
fn span(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

/// When something happened, as the person's clock says it: `14:02`, `yesterday 14:02`,
/// `Mon 14:02`, `12 Nov`.
fn when(at_ms: u64, now_ms: u64) -> String {
    let (at, now) = (local_or_utc((at_ms / 1000) as i64), local_or_utc((now_ms / 1000) as i64));
    match now.day_number() - at.day_number() {
        ..=0 => at.hm(),
        1 => format!("yesterday {}", at.hm()),
        2..=6 => format!("{} {}", WEEKDAY_ABBR[at.weekday() as usize], at.hm()),
        _ => format!("{} {}", at.day, MONTH_ABBR[(at.month.clamp(1, 12) - 1) as usize]),
    }
}

/// A head's line beside its state: who asked, and how long it waited or when it moved.
fn head_meta(from: &str, state: State, last_ms: u64, now_ms: u64) -> String {
    let at = match state {
        // (since it came to the person: its filing, or main's latest reply)
        State::Open if now_ms.saturating_sub(last_ms) < 60_000 => "just now".into(),
        State::Open => format!("waiting {}", span(now_ms.saturating_sub(last_ms))),
        State::Waiting => format!("since {}", when(last_ms, now_ms)),
        _ => when(last_ms, now_ms),
    };
    format!("{from} · {at}")
}

/// A message's two lines: who and when, then what.
fn message_lines(m: &Msg, option: &str, now_ms: u64) -> (String, String) {
    let time = when(m.at_ms, now_ms);
    if m.by == "main" { return (format!("main · {time}"), m.text.clone()); }
    let who = if m.kind == "ask" { "You asked" } else { "You answered" };
    let via = if m.source == "tray" { String::new() } else { format!(" · via {}", m.source) };
    let mut what = Vec::new();
    if m.choice.is_some() { what.push(format!("Chose \u{201c}{option}\u{201d}")); }
    if m.decide { what.push("Left it to main".into()); }
    if !m.text.is_empty() { what.push(m.text.clone()); }
    (format!("{who} · {time}{via}"), what.join(" \u{2014} "))
}

impl Inbox {
    /// What a row's widget shows. A ghost shows what it showed last, its controls off.
    fn fill(cx: &mut Cx2d, item: &WidgetRef, row: &Row, live: bool, model: &Model, now_ms: u64) {
        let pending = model.pending.get(&row.id).filter(|_| live);
        let busy = pending.is_some() || !live;
        match &row.kind {
            Kind::Header { open, waiting, history, note, status } => {
                item.view(cx, ids!(open)).set_visible(cx, *open > 0);
                item.view(cx, ids!(clear)).set_visible(cx, *open == 0);
                item.label(cx, ids!(open_count)).set_text(cx, &format!("{open} for you"));
                let mut parts = Vec::new();
                if *waiting > 0 { parts.push(format!("{waiting} with main")); }
                if *history > 0 { parts.push(format!("{history} in history")); }
                let summary = item.label(cx, ids!(summary));
                summary.set_visible(cx, !parts.is_empty());
                summary.set_text(cx, &parts.join(" \u{b7} "));
                let note_line = item.label(cx, ids!(note));
                note_line.set_visible(cx, !note.is_empty());
                note_line.set_text(cx, note);
                item.view(cx, ids!(status)).set_visible(cx, !status.is_empty());
                item.label(cx, ids!(status_text)).set_text(cx, status);
            }
            Kind::Section { count, clear } => {
                item.label(cx, ids!(text)).set_text(cx, row.section.title());
                item.label(cx, ids!(count)).set_text(cx, &count.to_string());
                item.button(cx, ids!(clear)).set_visible(cx, *clear && live);
            }
            Kind::Confirm { cleared, open, untaken, pending } => {
                let kept = kept_words(*open as u64, *untaken as u64);
                let text = match cleared {
                    0 => format!("Nothing to clear yet{kept}."),
                    n => format!("Clear {n} from history? They move to the archive{kept}."),
                };
                item.label(cx, ids!(text)).set_text(cx, &text);
                let confirm = item.button(cx, ids!(confirm));
                confirm.set_disabled(cx, !live || *pending || *cleared == 0);
                confirm.set_loading(cx, *pending);
                item.button(cx, ids!(cancel)).set_disabled(cx, !live || *pending);
            }
            Kind::Head { title, from, last_ms, state, refused, new, toggle } => {
                item.label(cx, ids!(title)).set_text(cx, title);
                for (chip, shown) in [
                    // (the turns say theirs by their section and stripe)
                    (ids!(chip_answered), *state == State::Answered), (ids!(chip_closed), *state == State::Closed),
                    (ids!(chip_refused), *refused), (ids!(chip_new), *new),
                ] {
                    item.view(cx, chip).set_visible(cx, shown);
                }
                item.label(cx, ids!(meta)).set_text(cx, &head_meta(from, *state, *last_ms, now_ms));
                let button = item.button(cx, ids!(toggle));
                button.set_visible(cx, toggle.is_some());
                button.set_text(cx, if *toggle == Some(true) { "Hide" } else { "Show" });
            }
            Kind::Notice(text) => item.label(cx, ids!(text)).set_text(cx, text),
            Kind::Field { label, text } => {
                item.label(cx, ids!(label)).set_text(cx, label);
                item.label(cx, ids!(text)).set_text(cx, text);
            }
            Kind::Context { shown } => item.button(cx, ids!(toggle)).set_text(cx, if *shown { "Hide context" } else { "Show context" }),
            Kind::ContextBody(text) => item.label(cx, ids!(text)).set_text(cx, text),
            Kind::Msg { message, option, brief } => {
                let (who, text) = message_lines(message, option, now_ms);
                let who_line = item.label(cx, ids!(who));
                who_line.set_visible(cx, !brief);
                who_line.set_text(cx, &who);
                // a folded card's line says who only when it was not the person's answer
                let text = match (*brief, message.by.as_str(), message.kind.as_str()) {
                    (true, "main", _) => format!("main: {}", message.text),
                    (true, _, "ask") => format!("You asked: {}", message.text),
                    _ => text,
                };
                let line = item.label(cx, ids!(text));
                line.set_visible(cx, !text.is_empty());
                line.set_text(cx, &text);
            }
            Kind::Option { number, text, recommended } => {
                // the recommended choice is filled and says so, the others are outlined
                for (path, shown) in [(ids!(pick), *recommended), (ids!(choose), !*recommended)] {
                    let button = item.button(cx, path);
                    button.set_visible(cx, shown);
                    button.set_text(cx, text);
                    button.set_disabled(cx, busy);
                    button.set_loading(cx, pending.is_some_and(|a| a.choice == Some(*number)));
                }
            }
            Kind::Decide => {
                let button = item.button(cx, ids!(decide));
                button.set_disabled(cx, busy);
                button.set_loading(cx, pending.is_some_and(|a| a.decide));
            }
            Kind::Input => {
                let draft = model.drafts.get(&row.id).map_or("", String::as_str);
                let input = item.text_input(cx, ids!(draft));
                if live && input.text() != draft { input.set_text(cx, draft); }
                for (path, ask) in [(ids!(answer), false), (ids!(ask), true)] {
                    let button = item.button(cx, path);
                    button.set_disabled(cx, busy || draft.trim().is_empty());
                    button.set_loading(cx, pending.is_some_and(|a| a.choice.is_none() && !a.decide && a.ask == ask));
                }
            }
            Kind::End | Kind::Gap | Kind::Empty => {}
        }
    }
}

impl Widget for Inbox {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync_rows(cx);
        let now = cx.seconds_since_app_start();
        self.settle(cx, now);
        if self.tick.0 == 0 { self.tick = cx.start_interval(30.0); }
        let model_ref = self.model.clone();
        let model = model_ref.borrow();
        let now_ms = makepad_devhub::proto::wall_ms();
        // Each moving block's height and each of its rows' place in it: a block shows the
        // lower part of its rows (they slide down out from under the row above as it opens).
        let mut totals: HashMap<u64, f64> = HashMap::new();
        let mut offsets = vec![0.0; self.slots.len()];
        for (i, slot) in self.slots.iter().enumerate() {
            if slot.block == 0 { continue; }
            let total = totals.entry(slot.block).or_default();
            offsets[i] = *total;
            *total += slot.height;
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                let mut drawn = HashMap::new();
                let mut shown = HashSet::new();
                list.set_item_range(cx, 0, self.slots.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(slot) = self.slots.get(index) else { continue };
                    let row = &slot.row;
                    let template = match &row.kind {
                        Kind::Header { .. } => id!(Header), Kind::Section { .. } => id!(Section), Kind::Confirm { .. } => id!(Confirm),
                        Kind::Head { .. } => id!(Head), Kind::Notice(_) => id!(Notice), Kind::Field { .. } => id!(Field),
                        Kind::Context { .. } => id!(Context), Kind::ContextBody(_) => id!(ContextBody),
                        Kind::Msg { .. } => id!(Msg), Kind::Option { .. } => id!(Option), Kind::Decide => id!(Decide),
                        Kind::Input => id!(Input), Kind::End => id!(End), Kind::Gap => id!(Gap), Kind::Empty => id!(Empty),
                    };
                    let (mut item, existed) = list.item_with_existed(cx, index, template);
                    if slot.live { drawn.insert(item.widget_uid(), row.clone()); }
                    shown.insert(item.widget_uid());
                    Self::fill(cx, &item, row, slot.live, &model, now_ms);
                    if row.on_card() {
                        let state = (row.section.tone(), self.glow(&row.id, now));
                        // A widget takes its stripe and glow at every draw while it has one (a new
                        // widget's shader is built at its first draw, and a reloaded design resets
                        // it), and once more when it goes back to plain.
                        let uid = item.widget_uid();
                        let plain = state == (0.0, 0.0);
                        if !plain || self.applied.get(&uid).is_some_and(|&was| was != state) || !existed {
                            if plain { self.applied.remove(&uid); } else { self.applied.insert(uid, state); }
                            let (tone, glow) = state;
                            script_apply_eval!(cx, item, { draw_bg.tone: #(tone) draw_bg.glow: #(glow) });
                        }
                    }
                    let reveal = self.reveal(slot.block, now);
                    if slot.block == 0 {
                        item.draw_all_unscoped(cx);
                    } else {
                        let total = totals.get(&slot.block).copied().unwrap_or(0.0);
                        let shown = reveal * total;
                        let top = offsets[index] - (total - shown);
                        let visible_top = top.max(0.0);
                        let visible = ((top + slot.height).min(shown) - visible_top).max(0.0);
                        cx.begin_turtle(
                            Walk { width: Size::fill(), height: Size::Fixed(visible), ..Walk::default() },
                            Layout { scroll: dvec2(0.0, visible_top - top), clip_x: true, clip_y: true, ..Layout::flow_down() },
                        );
                        item.draw_all_unscoped(cx);
                        cx.end_turtle();
                    }
                    let height = item.area().rect(cx).size.y;
                    if let Some(slot) = self.slots.get_mut(index) { slot.height = height; }
                }
                self.drawn = drawn;
                // A striped row takes its stripe at every draw anyway; only a glow must be
                // taken off where it was, so only those are kept for widgets out of sight.
                self.applied.retain(|uid, &mut (_, glow)| glow > 0.0 || shown.contains(uid));
            }
        }
        if !self.blocks.is_empty() || !self.glows.is_empty() {
            self.next_frame = cx.new_next_frame();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if self.next_frame.is_event(event).is_some() || self.tick.is_event(event).is_some() {
            self.view.redraw(cx);
        }
        self.sync_rows(cx);
        // Handle actions while the input's provenance is still on Cx.
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let list = self.view.portal_list(cx, ids!(list));
        let mut seen = HashSet::new();
        let mut changed = false;
        let mut rebuild = false;
        let model_ref = self.model.clone();
        let mut model = model_ref.borrow_mut();
        for (index, item) in list.items_with_actions(&actions) {
            if !seen.insert(index) { continue; }
            // A snapshot can arrive before the next draw. Route the press to the card
            // that was actually shown, even when new questions changed the row indices;
            // a row on its way out takes no presses.
            let Some(row) = self.drawn.get(&item.widget_uid()).cloned() else { continue };
            let mut send = None;
            match row.kind {
                Kind::Section { clear: true, .. } if item.button(cx, ids!(clear)).clicked(&actions) => {
                    model.clear.clearing = Clearing::Confirm;
                    model.clear.note = None;
                    changed = true;
                    rebuild = true;
                }
                Kind::Confirm { pending: false, .. } if item.button(cx, ids!(cancel)).clicked(&actions) => {
                    model.clear.clearing = Clearing::Idle;
                    changed = true;
                    rebuild = true;
                }
                Kind::Confirm { pending: false, cleared, .. } if cleared > 0 && item.button(cx, ids!(confirm)).clicked(&actions) => {
                    // the person's own press: injected input clears nothing
                    if let Some(by) = cx.injected_by() {
                        log!("aires: clear: not sent: injected input ({by})");
                        model.clear.clearing = Clearing::Idle;
                        model.clear.note = Some(format!("Not cleared: injected input ({by})"));
                        rebuild = true;
                    } else {
                        cx.widget_action(self.widget_uid(), InboxAction::Clear);
                    }
                    changed = true;
                }
                Kind::Head { toggle: Some(_), .. } if item.button(cx, ids!(toggle)).clicked(&actions) => {
                    if !model.expanded.remove(&row.id) { model.expanded.insert(row.id.clone()); }
                    changed = true;
                    rebuild = true;
                }
                Kind::Context { .. } if item.button(cx, ids!(toggle)).clicked(&actions) => {
                    let key = context_key(&row.id);
                    if !model.expanded.remove(&key) { model.expanded.insert(key); }
                    changed = true;
                    rebuild = true;
                }
                Kind::Option { number, .. } if item.button(cx, ids!(choose)).clicked(&actions) || item.button(cx, ids!(pick)).clicked(&actions) => {
                    send = Some((Some(number), false, false))
                }
                Kind::Decide if item.button(cx, ids!(decide)).clicked(&actions) => send = Some((None, true, false)),
                Kind::Input => {
                    if let Some(text) = item.text_input(cx, ids!(draft)).changed(&actions) {
                        model.drafts.insert(row.id.clone(), text);
                        changed = true;
                    }
                    if model.drafts.get(&row.id).is_some_and(|text| !text.trim().is_empty()) {
                        // (a typed question sent as an answer is a follow-up: main's reply gives
                        // the turn back, where an answer would leave it under the answered ones)
                        let question = model.drafts.get(&row.id).is_some_and(|text| text.trim_end().ends_with('?'));
                        if item.button(cx, ids!(answer)).clicked(&actions) { send = Some((None, false, question)); }
                        if item.button(cx, ids!(ask)).clicked(&actions) { send = Some((None, false, true)); }
                    }
                }
                _ => {}
            }
            if let Some((choice, decide, ask)) = send {
                if let Some(by) = cx.injected_by() {
                    let error = format!("not sent: injected input ({by})");
                    log!("aires: {}: {error}", row.id);
                    model.errors.insert(row.id, error);
                    changed = true;
                    rebuild = true;
                } else if !model.pending.contains_key(&row.id) {
                    let text = model.drafts.get(&row.id).cloned().unwrap_or_default();
                    cx.widget_action(self.widget_uid(), InboxAction::Answer(Answer { id: row.id, choice, decide, text, ask }));
                }
            }
        }
        if rebuild { model.rebuild(); }
        if changed { self.view.redraw(cx); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_devhub::ask::Refused;

    #[test]
    fn cards_follow_question_state_and_keep_the_thread_visible() {
        let mut questions = crate::source::demo_questions();
        let mut closed = questions.iter().find(|q| q.id == "Q1").unwrap().clone();
        closed.id = "Q9".into();
        closed.state = State::Closed;
        closed.refused.push(Refused { at_ms: 0, chain: "an untrusted sender".into(), why: "injected".into() });
        questions.push(closed);
        let mut expanded = HashSet::from(["Q9".into()]);
        let (errors, unseen) = (HashMap::new(), HashSet::from(["Q1".to_string()]));
        let idle = ClearView::default();
        let collapsed = rows(&questions, &expanded, &errors, &unseen, None, &idle);
        assert!(matches!(collapsed[0].kind, Kind::Header { open: 1, waiting: 1, history: 4, .. }), "the list opens with the counts per turn");
        let sections: Vec<_> = collapsed.iter().filter_map(|r| if let Kind::Section { count, clear } = r.kind { Some((r.section, count, clear)) } else { None }).collect();
        assert_eq!(sections, [(Section::Yours, 1, false), (Section::Main, 1, false), (Section::History, 4, true)], "sections follow the person's turn, main's turn, then history (answered and closed), which has the Clear link");
        let card = |rows: &[Row], id: &str| rows.iter().filter(|r| r.id == id && !matches!(r.kind, Kind::Gap | Kind::End)).cloned().collect::<Vec<_>>();
        let open = card(&collapsed, "Q1");
        assert!(matches!(open[0].kind, Kind::Head { new: true, refused: true, toggle: None, .. }), "the person's turn is a full card marked new and refused");
        assert!(matches!(&open[1].kind, Kind::Notice(text) if text.contains("Spotlight")), "the refusal says what the person does, under the card's head");
        assert_eq!(open.iter().filter(|r| matches!(r.kind, Kind::Option { .. })).count(), 3, "an open card has one row per option");
        assert_eq!(open.iter().filter(|r| matches!(r.kind, Kind::Option { recommended: true, number: 2, .. })).count(), 1, "the recommended option is marked");
        assert!(open.iter().any(|r| matches!(r.kind, Kind::Decide)) && open.iter().any(|r| matches!(r.kind, Kind::Input)), "an open card offers delegation and a draft");
        assert!(open.iter().any(|r| matches!(r.kind, Kind::Context { shown: false })) && !open.iter().any(|r| matches!(r.kind, Kind::ContextBody(_))), "the context is folded behind its switch");
        let closed = card(&collapsed, "Q9");
        assert!(!closed.iter().any(|r| matches!(r.kind, Kind::Option { .. } | Kind::Decide | Kind::Input | Kind::Context { .. })), "a closed card has no answer or context controls");
        assert!(closed.iter().any(|r| matches!(&r.kind, Kind::Notice(text) if text.contains("injected"))), "a refused answer is a line in its card, saying why");
        let waiting = card(&collapsed, "Q2");
        assert!(matches!(waiting.as_slice(), [Row { kind: Kind::Head { toggle: Some(false), .. }, .. }, Row { kind: Kind::Msg { .. }, .. }]), "main's turn folds to its last message");
        let answered = card(&collapsed, "Q3");
        assert!(matches!(answered.as_slice(), [Row { kind: Kind::Head { .. }, .. }, Row { kind: Kind::Msg { .. }, .. }]), "a folded answered card has its head and last answer");
        assert!(matches!(&answered[1].kind, Kind::Msg { message, .. } if message.text == "Keep the longer context behind Show all."), "the folded message is the last answer");
        expanded.insert("Q3".into());
        expanded.insert(context_key("Q1"));
        let full = rows(&questions, &expanded, &errors, &unseen, None, &idle);
        assert!(full.iter().any(|r| r.id == "Q3" && matches!(r.kind, Kind::Input)), "an expanded answered card permits another answer");
        assert!(full.iter().any(|r| r.id == "Q1" && matches!(&r.kind, Kind::ContextBody(text) if text.contains('\n'))), "a shown context keeps all its lines");
        assert!(matches!(rows(&[], &expanded, &errors, &unseen, None, &idle)[1].kind, Kind::Empty), "an empty snapshot has an empty row");
        // A question main reopened with a reply heads the person's turn, its reply in its card,
        // above one filed after it.
        let mut reopened = questions.clone();
        let mut newer = reopened.iter().find(|q| q.id == "Q1").unwrap().clone();
        let filed = last_ms(&newer) + 1_000;
        (newer.id, newer.created_ms, newer.thread) = ("Q7".into(), filed, Vec::new());
        reopened.insert(0, newer);
        let q1 = reopened.iter_mut().find(|q| q.id == "Q1").unwrap();
        q1.thread.push(Msg { seq: 9, at_ms: filed + 60_000, by: "main".into(), kind: "reopen".into(), choice: None, decide: false,
            text: "Which do you pick?".into(), source: "main".into(), chain: String::new(), taken: false });
        let turn = rows(&reopened, &HashSet::new(), &errors, &HashSet::new(), None, &idle);
        let heads: Vec<&str> = turn.iter().filter(|r| r.section == Section::Yours && matches!(r.kind, Kind::Head { .. })).map(|r| r.id.as_str()).collect();
        assert_eq!(heads, ["Q1", "Q7"], "the reopened question comes first");
        assert!(turn.iter().any(|r| r.id == "Q1" && matches!(&r.kind, Kind::Msg { message, .. } if message.kind == "reopen")), "with main's reply in its card");
        assert_eq!(head_meta("main", State::Open, 3_600_000, 3_660_000), "main \u{b7} waiting 1m", "it waits since the reopen");
        assert!(rows(&[], &expanded, &errors, &unseen, Some("master not reachable"), &idle).iter().all(|r| !matches!(r.kind, Kind::Empty)), "while the master is not reachable, the header says so instead");

        // The list's moves: what stays, what leaves where it was, what enters where it goes.
        let keys = |rows: &[Row]| rows.iter().map(Row::key).collect::<Vec<_>>();
        let (before, after) = (keys(&collapsed), keys(&full));
        let steps = merge(&before.iter().map(|k| (k, true)).collect::<Vec<_>>(), &after);
        let kept = steps.iter().filter(|s| matches!(s, Step::Keep(..))).count();
        let entered: Vec<_> = steps.iter().filter_map(|s| if let Step::Enter(j) = s { Some(&after[*j]) } else { None }).collect();
        let left: Vec<_> = steps.iter().filter_map(|s| if let Step::Leave(i) = s { Some(&before[*i]) } else { None }).collect();
        assert_eq!(kept + left.len(), before.len(), "every old row stays or leaves");
        assert_eq!(kept + entered.len(), after.len(), "every new row is kept or enters");
        assert!(entered.iter().any(|k| k.0 == "Q1" && k.2 == "context body") && entered.iter().any(|k| k.0 == "Q3" && k.2 == "input"), "shown context and an unfolded card enter");
        assert!(left.is_empty(), "an unfolded card keeps its summary line, now a message of its thread: {left:?}");
        let mut answered = questions.clone();
        answered.iter_mut().find(|q| q.id == "Q1").unwrap().state = State::Answered;
        let moved = keys(&rows(&answered, &HashSet::new(), &errors, &HashSet::new(), None, &idle));
        let steps = merge(&before.iter().map(|k| (k, true)).collect::<Vec<_>>(), &moved);
        assert!(steps.iter().any(|s| matches!(s, Step::Leave(i) if before[*i].0 == "Q1" && before[*i].2 == "decide")), "an answered card leaves the person's turn");
        assert!(steps.iter().any(|s| matches!(s, Step::Enter(j) if moved[*j].0 == "Q1" && moved[*j].1 == Section::History && moved[*j].2 == "head")), "and enters the history");
        // What is new: a turn that came after the first snapshot, until the person has looked.
        let mut model = Model::default();
        let snapshot = |questions: &[Question]| Some(Snapshot { questions: Some(questions.to_vec()), error: None });
        model.update(snapshot(&questions), Vec::new());
        assert!(model.unseen.is_empty(), "the turns at the first snapshot are not new");
        let mut later = questions.clone();
        let mut arrived = later.iter().find(|q| q.id == "Q1").unwrap().clone();
        arrived.id = "Q8".into();
        later.push(arrived);
        model.update(snapshot(&later), Vec::new());
        assert_eq!(model.unseen, HashSet::from(["Q8".to_string()]), "a turn that arrives later is new");
        assert!(model.rows.iter().any(|r| r.id == "Q8" && matches!(r.kind, Kind::Head { new: true, .. })), "its card says it is new");
        model.mark_seen();
        assert!(model.unseen.is_empty() && !model.rows.iter().any(|r| matches!(r.kind, Kind::Head { new: true, .. })), "once the person has looked, nothing is new");
        // The history's clear asks first, saying what goes and what stays: the answer main took
        // (Q4) and the closed ones (Q5, Q9) go; the open turns and Q3's untaken answer stay.
        let confirm = ClearView { clearing: Clearing::Confirm, note: None };
        let asking = rows(&questions, &HashSet::new(), &errors, &HashSet::new(), None, &confirm);
        let history = asking.iter().position(|r| matches!(r.kind, Kind::Section { .. }) && r.section == Section::History).unwrap();
        assert!(matches!(asking[history].kind, Kind::Section { clear: false, .. }), "the link gives way to its confirm");
        assert!(matches!(asking[history + 1].kind, Kind::Confirm { cleared: 3, open: 2, untaken: 1, pending: false }), "the confirm is under the history's line: {:?}", asking[history + 1].kind);
        model.cleared(Ok(Cleared { cleared: 3, open: 2, untaken: 1 }));
        assert!(matches!(&model.rows[0].kind, Kind::Header { note, .. } if note == "Cleared 3 to the archive; 2 open and 1 answer main has not taken stay"), "the header says what went and what stayed");
        model.mark_seen();
        assert!(matches!(&model.rows[0].kind, Kind::Header { note, .. } if note.is_empty()), "until the person has looked");
        // A ghost whose row comes back is kept, not doubled.
        let back = merge(&[(&before[1], false), (&before[2], true)], &before[1..3]);
        assert_eq!(back, [Step::Keep(0, 0), Step::Keep(1, 1)], "a row that comes back revives its ghost");
    }
}
