use crate::source::{Answer, AnswerResult, Snapshot};
use makepad_devhub::ask::{Msg, Question, State};
use makepad_widgets::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct Row {
    pub id: String,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub enum Kind {
    Section(String),
    Head { title: String, from: String, created_ms: u64, state: State },
    Field { label: &'static str, text: String },
    Context { text: String, shown: bool },
    Option { number: u32, text: String, recommended: bool },
    Decide,
    Input,
    Msg { message: Msg, option: String },
    Refused(String),
    Error(String),
    Gap,
    Empty(String),
}

type RowKey = (String, &'static str, u32);

impl Row {
    fn key(&self) -> RowKey {
        let (kind, number) = match &self.kind {
            Kind::Section(name) => ("section", match name.as_str() {
                "Waiting for main" => 1, "Answered" => 2, "Closed" => 3, _ => 0,
            }),
            Kind::Head { .. } => ("head", 0),
            Kind::Field { label, .. } => (*label, 0),
            Kind::Context { .. } => ("context", 0),
            Kind::Option { number, .. } => ("option", *number),
            Kind::Decide => ("decide", 0),
            Kind::Input => ("input", 0),
            Kind::Msg { message, .. } => ("message", message.seq),
            Kind::Refused(_) => ("refused", 0),
            Kind::Error(_) => ("error", 0),
            Kind::Gap => ("gap", 0),
            Kind::Empty(_) => ("empty", 0),
        };
        (self.id.clone(), kind, number)
    }
}

fn context_key(id: &str) -> String {
    format!("context:{id}")
}

fn message(q: &Question, m: &Msg) -> Kind {
    let option = m.choice.and_then(|n| n.checked_sub(1)).and_then(|n| q.options.get(n as usize)).cloned().unwrap_or_default();
    Kind::Msg { message: m.clone(), option }
}

pub fn rows(questions: &[Question], expanded: &HashSet<String>, errors: &HashMap<String, String>) -> Vec<Row> {
    let mut out = Vec::new();
    for state in [State::Open, State::Waiting, State::Answered, State::Closed] {
        let count = questions.iter().filter(|q| q.state == state).count();
        if count == 0 { continue; }
        let title = match state {
            State::Open => format!("Waiting for you ({count})"),
            State::Waiting => "Waiting for main".into(),
            State::Answered => "Answered".into(),
            State::Closed => "Closed".into(),
        };
        out.push(Row { id: String::new(), kind: Kind::Section(title) });
        // Keep the master's order within each section.
        for q in questions.iter().filter(|q| q.state == state) {
            let mut push = |kind| out.push(Row { id: q.id.clone(), kind });
            push(Kind::Head { title: q.title.clone(), from: q.from.clone(), created_ms: q.created_ms, state });
            let full = matches!(state, State::Open | State::Waiting) || expanded.contains(&q.id);
            if full {
                for (label, text) in [("Decided", &q.decided), ("Why now", &q.why), ("Blocked", &q.blocked), ("Recommended", &q.recommend)] {
                    if !text.is_empty() { push(Kind::Field { label, text: text.clone() }); }
                }
                if state != State::Closed {
                    if !q.context.is_empty() {
                        let shown = expanded.contains(&context_key(&q.id));
                        let text = if shown { q.context.clone() } else { q.context.lines().next().unwrap_or("").into() };
                        push(Kind::Context { text, shown });
                    }
                    for (i, text) in q.options.iter().enumerate() {
                        push(Kind::Option { number: (i + 1) as u32, text: text.clone(), recommended: *text == q.recommend });
                    }
                    push(Kind::Decide);
                    push(Kind::Input);
                }
                for m in &q.thread { push(message(q, m)); }
            } else if state == State::Answered {
                if let Some(m) = q.thread.iter().rev().find(|m| m.by == "user" && m.kind == "answer") {
                    push(message(q, m));
                }
            }
            // Refusals stay visible even when the card is collapsed.
            if !q.refused.is_empty() {
                let chains = q.refused.iter().map(|r| r.chain.as_str()).collect::<Vec<_>>().join("\n");
                push(Kind::Refused(format!("{} answer attempts were refused (not from this app):\n{chains}", q.refused.len())));
            }
            if let Some(error) = errors.get(&q.id) { push(Kind::Error(error.clone())); }
            push(Kind::Gap);
        }
    }
    if out.is_empty() { out.push(Row { id: String::new(), kind: Kind::Empty("No questions".into()) }); }
    out
}

#[derive(Default)]
pub struct Model {
    pub questions: Vec<Question>,
    pub rows: Vec<Row>,
    pub drafts: HashMap<String, String>,
    pub errors: HashMap<String, String>,
    pub pending: HashSet<String>,
    expanded: HashSet<String>,
    pub status: Option<String>,
    revision: u64,
}

impl Model {
    pub fn rebuild(&mut self) {
        self.revision += 1;
        self.rows = rows(&self.questions, &self.expanded, &self.errors);
        if let Some(status) = &self.status {
            let row = Row { id: String::new(), kind: Kind::Empty(status.clone()) };
            if self.questions.is_empty() { self.rows[0] = row; } else { self.rows.insert(0, row); }
        }
    }

    pub fn update(&mut self, snapshot: Option<Snapshot>, results: Vec<AnswerResult>) {
        if let Some(snapshot) = snapshot {
            if let Some(questions) = snapshot.questions { self.questions = questions; }
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
}

#[derive(Clone, Debug, Default)]
pub enum InboxAction {
    Answer(Answer),
    #[default]
    None,
}

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Text = Label{width: Fill height: Fit flow: Right{wrap: true}}
    let Row = View{
        width: Fill height: Fit flow: Down spacing: 6
        padding: Inset{left: 14 right: 14 top: 4 bottom: 4}
    }
    let WideButton = Button{
        width: Fill height: Fit
        flow: Right{wrap: true}
        label_walk: Walk{width: Fill height: Fit}
        label_align: Align{x: 0.0 y: 0.5}
    }
    mod.widgets.InboxBase = #(Inbox::register_widget(vm))
    mod.widgets.Inbox = set_type_default() do mod.widgets.InboxBase{
        width: Fill height: Fill
        list := PortalList{
            width: Fill height: Fill
            scroll_bar: ScrollBar{}
            Section := Row{
                padding: Inset{left: 14 right: 14 top: 16 bottom: 8}
                text := Text{draw_text.text_style: theme.font_bold{}}
            }
            Head := Row{
                title := ButtonFlatter{
                    width: Fill height: Fit
                    flow: Right{wrap: true}
                    label_walk: Walk{width: Fill height: Fit}
                    label_align: Align{x: 0.0 y: 0.5}
                    draw_text.text_style: theme.font_bold{}
                }
                meta := Text{}
            }
            Field := Row{
                label := Text{draw_text.text_style: theme.font_bold{}}
                text := Text{}
            }
            Context := Row{
                text := Text{}
                toggle := WideButton{text: "Show all"}
            }
            Option := Row{choose := WideButton{}}
            Decide := Row{decide := WideButton{text: "You decide"}}
            Input := Row{
                draft := TextInput{
                    width: Fill height: Fit
                    is_multiline: true
                    flow: Right{wrap: true}
                    wrap_empty_text: true
                    empty_text: "Your answer or follow-up question"
                }
                View{
                    width: Fill height: Fit flow: Right{wrap: true} spacing: 8
                    answer := Button{text: "Answer"}
                    ask := Button{text: "Ask"}
                }
            }
            Msg := Row{text := Text{}}
            Refused := Row{text := Text{draw_text.color: #xc98238}}
            Error := Row{text := Text{draw_text.color: #xc98238}}
            Gap := View{width: Fill height: 16}
            Empty := Row{
                padding: Inset{left: 14 right: 14 top: 18 bottom: 18}
                text := Text{}
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Inbox {
    #[deref]
    view: View,
    #[rust]
    pub model: Rc<RefCell<Model>>,
    #[rust]
    drawn: HashMap<WidgetUid, Row>,
    #[rust]
    keys: Vec<RowKey>,
    #[rust]
    revision: u64,
}

impl Inbox {
    fn sync_rows(&mut self, cx: &mut Cx) {
        let model = self.model.borrow();
        if self.revision == model.revision { return; }
        let list_ref = self.view.portal_list(cx, ids!(list));
        let Some(mut list) = list_ref.borrow_mut() else { return };
        self.revision = model.revision;
        let at_top = list.first_id() == 0 && list.first_scroll() >= -0.5;
        list.set_item_range(cx, 0, self.keys.len());
        let keys: Vec<_> = model.rows.iter().map(Row::key).collect();
        let positions: HashMap<_, _> = keys.iter().enumerate().map(|(i, key)| (key, i)).collect();
        // Splice insertions and removals so existing inputs keep their focus and cursor.
        // Removed controls lose their captures through PortalList's own splice handling.
        let (mut old, mut next) = (0, 0);
        while old < self.keys.len() {
            match positions.get(&self.keys[old]).copied().filter(|&i| i >= next) {
                Some(at) => {
                    if at > next { list.splice_items(cx, next, 0, at - next); }
                    next = at + 1;
                    old += 1;
                }
                None => {
                    let start = old;
                    while old < self.keys.len() && !positions.get(&self.keys[old]).is_some_and(|&i| i >= next) { old += 1; }
                    list.splice_items(cx, next, old - start, 0);
                }
            }
        }
        if next < keys.len() { list.splice_items(cx, next, 0, keys.len() - next); }
        if at_top {
            list.set_first_id_and_scroll(0, 0.0);
        } else if list.first_id() >= keys.len() {
            list.set_first_id_and_scroll(keys.len().saturating_sub(1), 0.0);
        }
        self.keys = keys;
    }
}

fn age(created_ms: u64, now: u64) -> String {
    let seconds = now.saturating_sub(created_ms) / 1000;
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn message_text(m: &Msg, option: &str) -> String {
    let time = makepad_civil_time::local_or_utc((m.at_ms / 1000) as i64).hm();
    if m.by == "main" { return format!("main {time}: {}", m.text); }
    let mut parts = Vec::new();
    if let Some(choice) = m.choice { parts.push(format!("option {choice} {}", makepad_remote::json_str(option))); }
    if m.decide { parts.push("You decide".into()); }
    if m.kind == "ask" { parts.push("follow-up".into()); }
    if !m.text.is_empty() { parts.push(m.text.clone()); }
    format!("You ({}) {time}: {}", m.source, parts.join(" - "))
}

impl Widget for Inbox {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.sync_rows(cx);
        let model = self.model.borrow();
        let now = makepad_devhub::proto::wall_ms();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                let mut drawn = HashMap::new();
                list.set_item_range(cx, 0, model.rows.len());
                while let Some(index) = list.next_visible_item(cx) {
                    let Some(row) = model.rows.get(index) else { continue };
                    let template = match &row.kind {
                        Kind::Section(_) => id!(Section), Kind::Head { .. } => id!(Head),
                        Kind::Field { .. } => id!(Field), Kind::Context { .. } => id!(Context),
                        Kind::Option { .. } => id!(Option), Kind::Decide => id!(Decide), Kind::Input => id!(Input),
                        Kind::Msg { .. } => id!(Msg), Kind::Refused(_) => id!(Refused), Kind::Error(_) => id!(Error),
                        Kind::Gap => id!(Gap), Kind::Empty(_) => id!(Empty),
                    };
                    let item = list.item(cx, index, template);
                    drawn.insert(item.widget_uid(), row.clone());
                    let pending = model.pending.contains(&row.id);
                    match &row.kind {
                        Kind::Section(text) | Kind::Refused(text) | Kind::Error(text) | Kind::Empty(text) => item.label(cx, ids!(text)).set_text(cx, text),
                        Kind::Head { title, from, created_ms, state } => {
                            item.button(cx, ids!(title)).set_text(cx, title);
                            let hint = if matches!(state, State::Answered | State::Closed) {
                                if model.expanded.contains(&row.id) { " - Hide" } else { " - Show all" }
                            } else { "" };
                            item.label(cx, ids!(meta)).set_text(cx, &format!("from {from} - {} - {}{hint}", age(*created_ms, now), state.word()));
                        }
                        Kind::Field { label, text } => {
                            item.label(cx, ids!(label)).set_text(cx, label);
                            item.label(cx, ids!(text)).set_text(cx, text);
                        }
                        Kind::Context { text, shown } => {
                            item.label(cx, ids!(text)).set_text(cx, text);
                            item.button(cx, ids!(toggle)).set_text(cx, if *shown { "Hide" } else { "Show all" });
                        }
                        Kind::Option { number, text, recommended } => {
                            let button = item.button(cx, ids!(choose));
                            button.set_text(cx, &format!("{number}. {text}{}", if *recommended { " (recommended)" } else { "" }));
                            button.set_disabled(cx, pending);
                        }
                        Kind::Decide => item.button(cx, ids!(decide)).set_disabled(cx, pending),
                        Kind::Input => {
                            let draft = model.drafts.get(&row.id).map_or("", String::as_str);
                            let input = item.text_input(cx, ids!(draft));
                            if input.text() != draft { input.set_text(cx, draft); }
                            for button in [ids!(answer), ids!(ask)] {
                                item.button(cx, button).set_disabled(cx, pending || draft.trim().is_empty());
                            }
                        }
                        Kind::Msg { message, option } => item.label(cx, ids!(text)).set_text(cx, &message_text(message, option)),
                        Kind::Gap => {}
                    }
                    item.draw_all_unscoped(cx);
                }
                self.drawn = drawn;
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.sync_rows(cx);
        // Handle actions while the input's provenance is still on Cx.
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        let list = self.view.portal_list(cx, ids!(list));
        let mut seen = HashSet::new();
        let mut changed = false;
        let mut rebuild = false;
        let mut model = self.model.borrow_mut();
        for (index, item) in list.items_with_actions(&actions) {
            if !seen.insert(index) { continue; }
            // A snapshot can arrive before the next draw. Route the press to the card
            // that was actually shown, even when new questions changed the row indices.
            let Some(row) = self.drawn.get(&item.widget_uid()).cloned() else { continue };
            let mut send = None;
            match row.kind {
                Kind::Head { state: State::Answered | State::Closed, .. } if item.button(cx, ids!(title)).clicked(&actions) => {
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
                Kind::Option { number, .. } if item.button(cx, ids!(choose)).clicked(&actions) => send = Some((Some(number), false, false)),
                Kind::Decide if item.button(cx, ids!(decide)).clicked(&actions) => send = Some((None, true, false)),
                Kind::Input => {
                    if let Some(text) = item.text_input(cx, ids!(draft)).changed(&actions) {
                        model.drafts.insert(row.id.clone(), text);
                        changed = true;
                    }
                    if model.drafts.get(&row.id).is_some_and(|text| !text.trim().is_empty()) {
                        if item.button(cx, ids!(answer)).clicked(&actions) { send = Some((None, false, false)); }
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
                } else if !model.pending.contains(&row.id) {
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
        closed.id = "Q4".into();
        closed.state = State::Closed;
        closed.refused.push(Refused { at_ms: 0, chain: "an untrusted sender".into(), why: "injected".into() });
        questions.push(closed);
        let mut expanded = HashSet::from(["Q4".into()]);
        let errors = HashMap::new();
        let collapsed = rows(&questions, &expanded, &errors);
        let sections: Vec<_> = collapsed.iter().filter_map(|r| if let Kind::Section(s) = &r.kind { Some(s.as_str()) } else { None }).collect();
        assert_eq!(sections, ["Waiting for you (1)", "Waiting for main", "Answered", "Closed"], "sections follow the user's turn, main's turn, answered, closed");
        let card = |id: &str| collapsed.iter().filter(|r| r.id == id && !matches!(r.kind, Kind::Gap)).collect::<Vec<_>>();
        let open = card("Q1");
        assert_eq!(open.iter().filter(|r| matches!(r.kind, Kind::Option { .. })).count(), 3, "an open card has one row per option");
        assert_eq!(open.iter().filter(|r| matches!(r.kind, Kind::Option { recommended: true, number: 2, .. })).count(), 1, "the recommended option is marked");
        assert!(open.iter().any(|r| matches!(r.kind, Kind::Decide)) && open.iter().any(|r| matches!(r.kind, Kind::Input)), "an open card offers delegation and a draft");
        let closed = card("Q4");
        assert!(!closed.iter().any(|r| matches!(r.kind, Kind::Option { .. } | Kind::Decide | Kind::Input | Kind::Context { .. })), "a closed card has no answer or context controls");
        assert!(closed.iter().any(|r| matches!(&r.kind, Kind::Refused(text) if text.contains("an untrusted sender"))), "refused attempts name their chain");
        let answered = card("Q3");
        assert!(matches!(answered.as_slice(), [Row { kind: Kind::Head { .. }, .. }, Row { kind: Kind::Msg { .. }, .. }]), "a collapsed answered card has just its head and last answer");
        assert!(matches!(&answered[1].kind, Kind::Msg { message, .. } if message.text == "Keep the longer context behind Show all."), "the collapsed message is the last answer");
        expanded.insert("Q3".into());
        expanded.insert(context_key("Q1"));
        let full = rows(&questions, &expanded, &errors);
        assert!(full.iter().any(|r| r.id == "Q3" && matches!(r.kind, Kind::Input)), "an expanded answered card permits another answer");
        assert!(full.iter().any(|r| r.id == "Q1" && matches!(&r.kind, Kind::Context { text, shown: true } if text.contains('\n'))), "context expansion preserves all lines");
        assert!(matches!(&rows(&[], &expanded, &errors)[0].kind, Kind::Empty(text) if text == "No questions"), "an empty snapshot has an empty row");
    }
}
