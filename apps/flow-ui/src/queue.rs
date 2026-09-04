//! The run queue panel and its event-fed model. Queue rows are keyed by run
//! id; no timer or HTTP poll is needed to advance their state.

use crate::panels::RunBar;
use makepad_flow::{
    CreateBatchResponse, Event as FlowEvent, Literal, NodeRowDto, NodeState, RunRowDto, RunState,
    ValueRef,
};
use makepad_widgets::*;
use std::collections::{HashMap, HashSet};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.QueueListBase = #(QueueList::register_widget(vm))
    mod.widgets.QueueList = set_type_default() do mod.widgets.QueueListBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_1
        tools := View{
            width: Fill
            height: Fit
            flow: Right
            align: Align{x: 1.0 y: 0.5}
            clear_all := ButtonFlatter{text: "Clear all"}
        }
        hint := Label{
            width: Fill
            height: Fit
            margin: Inset{top: 6}
            text: "The queue is empty. Ctrl+Enter adds a batch."
            draw_text +: {
                color: theme.flow_text_hint
                text_style: theme.font_regular{font_size: 9}
            }
        }
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Header := View{
                width: Fill
                height: 22
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 6 right: 2}
                spacing: 6
                title := Label{
                    width: Fill
                    height: Fit
                    draw_text +: {
                        color: theme.flow_text_muted
                        text_style: theme.font_bold{font_size: 8}
                    }
                }
                cancel_batch := ButtonFlatter{
                    width: 18 height: 18 text: ""
                    padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                    icon_walk: Walk{width: 8 height: 8}
                    draw_icon +: {
                        svg: crate_resource("self:resources/icons/close.svg")
                        color: theme.flow_text_muted
                    }
                }
            }
            // One run: its name, a thin strip in the state's colour, the
            // state and time in words, then its x. Everything sits on one
            // centre line.
            Run := RoundedView{
                width: Fill
                height: 26
                flow: Right
                align: Align{y: 0.5}
                padding: Inset{left: 4 right: 4}
                spacing: 8
                cursor: MouseCursor.Hand
                capture_overload: true
                show_bg: true
                draw_bg +: {color: theme.flow_surface border_radius: 6}
                select := ButtonFlatter{
                    width: 56 height: 20 text: "#1"
                    padding: Inset{left: 2 right: 2 top: 0 bottom: 0}
                    align: Align{x: 0.0 y: 0.5}
                    draw_text +: {
                        color: theme.flow_text
                        text_style: theme.font_bold{font_size: 8.5}
                    }
                }
                progress := RunBar{width: Fill height: Fill thickness: 4}
                meta := Label{
                    width: 80 height: Fit text: "queued"
                    draw_text +: {
                        color: theme.flow_text_muted
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
                asset := ButtonFlatter{
                    width: Fit height: 18 text: "asset" visible: false
                    padding: Inset{left: 2 right: 2 top: 0 bottom: 0}
                    draw_text +: {
                        color: theme.flow_highlight
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
                cancel_run := ButtonFlatter{
                    width: 18 height: 18 text: ""
                    padding: Inset{left: 0 right: 0 top: 0 bottom: 0}
                    icon_walk: Walk{width: 8 height: 8}
                    draw_icon +: {
                        svg: crate_resource("self:resources/icons/close.svg")
                        color: theme.flow_text_muted
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum QueueAction {
    Select {
        run_id: String,
        instance: String,
        flow: String,
        revision: u64,
        state: RunState,
        planned_nodes: Vec<String>,
        started_ms: u64,
        finished_ms: Option<u64>,
    },
    CancelRun {
        run_id: String,
        instance: String,
        /// A batch slice keeps its siblings; a single run is stopped whole.
        batch: bool,
    },
    CancelBatch(String),
    ClearAll(ClearPlan),
    OpenAsset(String),
}

/// What `Clear all` asks the server for: every batch is cleared as one, and
/// each single run (a Play run) is stopped the way its own `x` stops it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClearPlan {
    pub batches: Vec<String>,
    /// `(run id, instance)` pairs.
    pub singles: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueRun {
    pub run_id: String,
    pub instance: String,
    pub flow: String,
    /// The batch this slice belongs to; a Play run has none.
    pub batch: Option<String>,
    pub index: u64,
    pub revision: u64,
    pub state: RunState,
    pub planned_nodes: Vec<String>,
    pub nodes: HashMap<String, NodeRowDto>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub asset: Option<String>,
}

impl QueueRun {
    fn from_row(row: RunRowDto) -> Self {
        let asset = asset_from_nodes(&row.nodes);
        Self {
            batch: row.batch.clone(),
            run_id: row.run_id,
            instance: row.instance,
            flow: row.flow,
            index: row.batch_index.unwrap_or(1),
            revision: row.revision,
            state: row.state,
            planned_nodes: row.planned_nodes,
            nodes: row.nodes,
            started_ms: row.started_ms,
            finished_ms: row.finished_ms,
            asset,
        }
    }

    pub fn permille(&self) -> u16 {
        if self.state == RunState::Done {
            return 1000;
        }
        if self.planned_nodes.is_empty() {
            return 0;
        }
        let total: u64 = self
            .planned_nodes
            .iter()
            .map(|node| {
                self.nodes.get(node).map_or(0, |row| match row.state {
                    NodeState::Done | NodeState::Skipped => 1000,
                    _ => u64::from(row.progress.unwrap_or(0)),
                })
            })
            .sum();
        (total / self.planned_nodes.len() as u64).min(1000) as u16
    }

    fn terminal(&self) -> bool {
        matches!(
            self.state,
            RunState::Done | RunState::Failed | RunState::Cancelled
        )
    }
}

/// One group of rows: a batch under its header, or a single Play run with
/// no header (`id: None`).
#[derive(Clone, Debug, PartialEq)]
pub struct QueueBatch {
    pub id: Option<String>,
    pub runs: Vec<QueueRun>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueModel {
    pub batches: Vec<QueueBatch>,
    pub selected: Option<String>,
    /// Single runs the user cleared or stopped: the server keeps their rows
    /// for a while, and a snapshot must not bring them back.
    hidden: HashSet<String>,
}

impl QueueModel {
    pub fn add_batch(
        &mut self,
        flow: &str,
        response: CreateBatchResponse,
        planned_nodes: &[String],
        revision: u64,
        now_ms: u64,
    ) {
        let runs = response
            .runs
            .into_iter()
            .enumerate()
            .map(|(offset, run)| QueueRun {
                run_id: run.run_id,
                instance: run.instance,
                flow: flow.to_string(),
                batch: Some(response.batch.clone()),
                index: offset as u64 + 1,
                revision,
                state: RunState::Queued,
                planned_nodes: planned_nodes.to_vec(),
                nodes: HashMap::new(),
                started_ms: now_ms,
                finished_ms: None,
                asset: None,
            })
            .collect();
        self.batches
            .retain(|batch| batch.id.as_deref() != Some(response.batch.as_str()));
        self.batches.insert(
            0,
            QueueBatch {
                id: Some(response.batch),
                runs,
            },
        );
    }

    /// Every run the server lists, newest first: batch slices under their
    /// header, Play runs on their own.
    pub fn set_rows(&mut self, rows: Vec<RunRowDto>) {
        let selected = self.selected.clone();
        let listed: HashSet<&str> = rows.iter().map(|row| row.run_id.as_str()).collect();
        self.hidden.retain(|run_id| listed.contains(run_id.as_str()));
        let mut rows: Vec<RunRowDto> = rows
            .into_iter()
            .filter(|row| !self.hidden.contains(&row.run_id))
            .collect();
        rows.sort_by(|left, right| {
            right
                .started_ms
                .cmp(&left.started_ms)
                .then_with(|| left.batch_index.cmp(&right.batch_index))
        });
        let mut batches = Vec::<QueueBatch>::new();
        for row in rows {
            let run = QueueRun::from_row(row);
            match run.batch.clone() {
                Some(id) => {
                    if let Some(batch) = batches
                        .iter_mut()
                        .find(|batch| batch.id.as_deref() == Some(id.as_str()))
                    {
                        batch.runs.push(run);
                    } else {
                        batches.push(QueueBatch {
                            id: Some(id),
                            runs: vec![run],
                        });
                    }
                }
                None => batches.push(QueueBatch {
                    id: None,
                    runs: vec![run],
                }),
            }
        }
        for batch in &mut batches {
            batch.runs.sort_by_key(|run| run.index);
        }
        self.batches = batches;
        self.selected = selected.filter(|selected| self.run(selected).is_some());
    }

    pub fn apply_event(&mut self, event: &FlowEvent, now_ms: u64) -> bool {
        let Some(run_id) = event.run_id.as_deref() else {
            return false;
        };
        let Some(run) = self.run_mut(run_id) else {
            return false;
        };
        let node = event.node.as_deref().unwrap_or_default();
        match event.kind.as_str() {
            "run.started" => {
                run.state = RunState::Running;
                run.started_ms = now_ms;
                if let Some(planned) = event.planned_nodes.as_ref() {
                    run.planned_nodes.clone_from(planned);
                }
            }
            "node.started" => set_node_state(run, node, NodeState::Running, Some(0)),
            "node.progress" => set_node_state(
                run,
                node,
                NodeState::Running,
                Some(event.permille.unwrap_or(0).min(1000) as u16),
            ),
            "node.waiting" => {
                run.state = RunState::Waiting;
                set_node_state(run, node, NodeState::Waiting, None);
            }
            "node.answered" => {
                run.state = RunState::Running;
                set_node_state(run, node, NodeState::Running, None);
            }
            "node.done" => {
                let outputs = event.output_values();
                if let Some(asset) = outputs
                    .iter()
                    .find(|(port, _)| port == "asset")
                    .map(|(_, value)| asset_text(value))
                {
                    run.asset = Some(asset);
                }
                let row = run.nodes.entry(node.to_string()).or_insert_with(empty_node);
                row.state = NodeState::Done;
                row.progress = Some(1000);
                row.outputs = outputs
                    .into_iter()
                    .map(|(port, value)| makepad_flow::PortValueRef { port, value })
                    .collect();
            }
            "node.failed" => set_node_state(run, node, NodeState::Failed, None),
            "node.skipped" => set_node_state(run, node, NodeState::Skipped, Some(1000)),
            "run.finished" => {
                run.state = event
                    .state_text()
                    .as_deref()
                    .and_then(parse_run_state)
                    .unwrap_or(RunState::Done);
                run.finished_ms = Some(now_ms);
            }
            _ => return false,
        }
        true
    }

    pub fn select(&mut self, run_id: &str) -> bool {
        if self.run(run_id).is_none() {
            return false;
        }
        self.selected = Some(run_id.to_string());
        true
    }

    /// A stopped single run leaves the list now and stays out of later
    /// snapshots.
    pub fn hide_run(&mut self, run_id: &str) {
        self.hidden.insert(run_id.to_string());
        for batch in &mut self.batches {
            batch.runs.retain(|run| run.run_id != run_id);
        }
        self.batches.retain(|batch| !batch.runs.is_empty());
        if self.selected.as_deref() == Some(run_id) {
            self.selected = None;
        }
    }

    pub fn clear_all(&mut self) -> ClearPlan {
        let mut plan = ClearPlan::default();
        for batch in self.batches.drain(..) {
            match batch.id {
                Some(id) => plan.batches.push(id),
                None => {
                    for run in batch.runs {
                        self.hidden.insert(run.run_id.clone());
                        plan.singles.push((run.run_id, run.instance));
                    }
                }
            }
        }
        self.selected = None;
        plan
    }

    pub fn run(&self, run_id: &str) -> Option<&QueueRun> {
        self.batches
            .iter()
            .flat_map(|batch| &batch.runs)
            .find(|run| run.run_id == run_id)
    }

    fn run_mut(&mut self, run_id: &str) -> Option<&mut QueueRun> {
        self.batches
            .iter_mut()
            .flat_map(|batch| &mut batch.runs)
            .find(|run| run.run_id == run_id)
    }

    fn items(&self) -> Vec<QueueItem> {
        let mut items = Vec::new();
        for batch in &self.batches {
            if let Some(id) = batch.id.as_ref() {
                items.push(QueueItem::Header {
                    id: id.clone(),
                    runs: batch.runs.len(),
                    done: batch.runs.iter().filter(|run| run.terminal()).count(),
                });
            }
            items.extend(batch.runs.iter().cloned().map(QueueItem::Run));
        }
        items
    }
}

#[derive(Clone, Debug)]
enum QueueItem {
    Header { id: String, runs: usize, done: usize },
    Run(QueueRun),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunRowHit {
    Select,
    Cancel,
    Asset,
}

fn resolve_run_row_hit(cancel: bool, asset: bool, select: bool) -> Option<RunRowHit> {
    if cancel {
        Some(RunRowHit::Cancel)
    } else if asset {
        Some(RunRowHit::Asset)
    } else {
        select.then_some(RunRowHit::Select)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct QueueList {
    #[deref]
    view: View,
    #[rust]
    model: QueueModel,
    #[rust]
    now_ms: u64,
    /// Instance → label (`run #3`), the name a single run's row shows.
    #[rust]
    labels: HashMap<String, String>,
}

impl QueueList {
    pub fn add_batch(
        &mut self,
        cx: &mut Cx,
        flow: &str,
        response: CreateBatchResponse,
        planned_nodes: &[String],
        revision: u64,
        now_ms: u64,
    ) {
        self.now_ms = now_ms;
        self.model
            .add_batch(flow, response, planned_nodes, revision, now_ms);
        self.sync_empty(cx);
    }

    pub fn set_rows(&mut self, cx: &mut Cx, rows: Vec<RunRowDto>) {
        self.now_ms = wall_clock_ms();
        self.model.set_rows(rows);
        self.sync_empty(cx);
    }

    pub fn set_now(&mut self, cx: &mut Cx, now_ms: u64) {
        self.now_ms = now_ms;
        if self
            .model
            .batches
            .iter()
            .flat_map(|batch| &batch.runs)
            .any(|run| !run.terminal())
        {
            self.redraw(cx);
        }
    }

    pub fn set_labels(&mut self, cx: &mut Cx, labels: HashMap<String, String>) {
        if self.labels != labels {
            self.labels = labels;
            self.redraw(cx);
        }
    }

    /// Feed one event to its row; returns whether the queue knew the run
    /// (an unknown run's row is the caller's to fetch).
    pub fn apply_event(&mut self, cx: &mut Cx, event: &FlowEvent, now_ms: u64) -> bool {
        self.now_ms = now_ms;
        let known = event
            .run_id
            .as_deref()
            .is_none_or(|run_id| self.model.run(run_id).is_some());
        if self.model.apply_event(event, now_ms) {
            self.redraw(cx);
        }
        known
    }

    pub fn select(&mut self, cx: &mut Cx, run_id: &str) {
        if self.model.select(run_id) {
            self.redraw(cx);
        }
    }

    /// No row is the shown run any more (the canvas went back to design).
    pub fn deselect(&mut self, cx: &mut Cx) {
        if self.model.selected.take().is_some() {
            self.redraw(cx);
        }
    }

    pub fn actions(&mut self, cx: &mut Cx, actions: &Actions) -> Vec<QueueAction> {
        let mut out = Vec::new();
        if self.view.button(cx, ids!(clear_all)).clicked(actions) {
            out.push(QueueAction::ClearAll(self.model.clear_all()));
            self.sync_empty(cx);
        }
        let items = self.model.items();
        let list = self.view.portal_list(cx, ids!(list));
        for (index, item) in list.items_with_actions(actions) {
            let Some(row) = items.get(index) else {
                continue;
            };
            match row {
                QueueItem::Header { id, .. } => {
                    if item.button(cx, ids!(cancel_batch)).clicked(actions) {
                        out.push(QueueAction::CancelBatch(id.clone()));
                    }
                }
                QueueItem::Run(run) => {
                    let row_clicked = item.as_view().finger_up(actions).is_some_and(|up| {
                        up.is_primary_hit() && up.is_over && up.was_tap()
                    });
                    let hit = resolve_run_row_hit(
                        item.button(cx, ids!(cancel_run)).clicked(actions),
                        item.button(cx, ids!(asset)).clicked(actions),
                        row_clicked,
                    );
                    match hit {
                        Some(RunRowHit::Select) => {
                            self.model.select(&run.run_id);
                            out.push(QueueAction::Select {
                                run_id: run.run_id.clone(),
                                instance: run.instance.clone(),
                                flow: run.flow.clone(),
                                revision: run.revision,
                                state: run.state,
                                planned_nodes: run.planned_nodes.clone(),
                                started_ms: run.started_ms,
                                finished_ms: run.finished_ms,
                            });
                        }
                        Some(RunRowHit::Cancel) => {
                            if run.batch.is_none() {
                                self.model.hide_run(&run.run_id);
                                self.sync_empty(cx);
                            }
                            out.push(QueueAction::CancelRun {
                                run_id: run.run_id.clone(),
                                instance: run.instance.clone(),
                                batch: run.batch.is_some(),
                            });
                        }
                        Some(RunRowHit::Asset) => {
                            if let Some(asset) = run.asset.clone() {
                                out.push(QueueAction::OpenAsset(asset));
                            }
                        }
                        None => {}
                    }
                }
            }
        }
        out
    }

    fn sync_empty(&mut self, cx: &mut Cx) {
        self.view
            .label(cx, ids!(hint))
            .set_visible(cx, self.model.batches.is_empty());
        self.view
            .button(cx, ids!(clear_all))
            .set_enabled(cx, !self.model.batches.is_empty());
        self.redraw(cx);
    }
}

impl Widget for QueueList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let items = self.model.items();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else {
                continue;
            };
            list.set_item_range(cx, 0, items.len());
            while let Some(index) = list.next_visible_item(cx) {
                let Some(row) = items.get(index) else {
                    continue;
                };
                match row {
                    QueueItem::Header { id, runs, done } => {
                        let item = list.item(cx, index, id!(Header));
                        item.label(cx, ids!(title)).set_text(
                            cx,
                            &format!("batch {} · {runs} runs · {done} done", short(id)),
                        );
                        item.draw_all_unscoped(cx);
                    }
                    QueueItem::Run(run) => {
                        let item = list.item(cx, index, id!(Run));
                        let selected = self.model.selected.as_deref() == Some(run.run_id.as_str());
                        let name = match run.batch {
                            Some(_) => format!("#{}", run.index),
                            None => self
                                .labels
                                .get(&run.instance)
                                .cloned()
                                .unwrap_or_else(|| "run".to_string()),
                        };
                        item.button(cx, ids!(select)).set_text(
                            cx,
                            &format!("{}{name}", if selected { "› " } else { "" }),
                        );
                        let state = state_name(run.state);
                        if let Some(mut bar) = item.widget(cx, ids!(progress)).borrow_mut::<RunBar>() {
                            bar.set_progress(cx, f64::from(run.permille()) / 1000.0, state);
                        }
                        let end = run.finished_ms.unwrap_or(self.now_ms);
                        let elapsed = format_elapsed(end.saturating_sub(run.started_ms));
                        let meta = match run.state {
                            RunState::Queued | RunState::Waiting => state.to_string(),
                            _ => format!("{state} · {elapsed}"),
                        };
                        item.label(cx, ids!(meta)).set_text(cx, &meta);
                        item.button(cx, ids!(asset))
                            .set_visible(cx, run.asset.is_some() && run.terminal());
                        // A batch slice's x cancels it, so it goes quiet once
                        // the slice is over; a single run's x is Stop, which
                        // also removes a finished run.
                        item.button(cx, ids!(cancel_run))
                            .set_enabled(cx, run.batch.is_none() || !run.terminal());
                        item.draw_all_unscoped(cx);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

fn empty_node() -> NodeRowDto {
    NodeRowDto {
        state: NodeState::Pending,
        progress: None,
        stage: None,
        outputs: Vec::new(),
        error: None,
        text: None,
    }
}

fn set_node_state(run: &mut QueueRun, node: &str, state: NodeState, progress: Option<u16>) {
    let row = run.nodes.entry(node.to_string()).or_insert_with(empty_node);
    row.state = state;
    if progress.is_some() {
        row.progress = progress;
    }
}

fn asset_from_nodes(nodes: &HashMap<String, NodeRowDto>) -> Option<String> {
    nodes.values().find_map(|node| {
        node.outputs
            .iter()
            .find(|output| output.port == "asset")
            .map(|output| asset_text(&output.value))
    })
}

fn asset_text(value: &ValueRef) -> String {
    match value.preview.as_ref() {
        Some(Literal::Str(text)) => text.clone(),
        _ => value.digest.clone(),
    }
}

fn parse_run_state(value: &str) -> Option<RunState> {
    Some(match value {
        "queued" => RunState::Queued,
        "running" => RunState::Running,
        "waiting" => RunState::Waiting,
        "done" => RunState::Done,
        "failed" => RunState::Failed,
        "cancelled" => RunState::Cancelled,
        _ => return None,
    })
}

fn state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Waiting => "waiting",
        RunState::Done => "done",
        RunState::Failed => "failed",
        RunState::Cancelled => "cancelled",
    }
}

fn short(value: &str) -> &str {
    value.get(..value.len().min(8)).unwrap_or(value)
}

fn format_elapsed(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m", seconds / 60)
    }
}

fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_flow::{BatchRunDto, CreateBatchResponse};

    fn batch() -> CreateBatchResponse {
        CreateBatchResponse {
            batch: "a3f2".into(),
            runs: vec![
                BatchRunDto {
                    run_id: "run-1".into(),
                    instance: "instance-1".into(),
                },
                BatchRunDto {
                    run_id: "run-2".into(),
                    instance: "instance-2".into(),
                },
            ],
        }
    }

    fn event(run: &str, kind: &str) -> FlowEvent {
        FlowEvent {
            topic: "run".into(),
            kind: kind.into(),
            run_id: Some(run.into()),
            ..FlowEvent::default()
        }
    }

    #[test]
    fn events_update_only_the_keyed_row_and_selection_sticks() {
        let mut model = QueueModel::default();
        model.add_batch("demo", batch(), &["gen".into()], 7, 1_000);
        assert!(model.select("run-1"));
        let mut progress = event("run-2", "node.progress");
        progress.node = Some("gen".into());
        progress.permille = Some(420);
        assert!(model.apply_event(&progress, 2_000));
        assert_eq!(model.run("run-2").unwrap().permille(), 420);
        assert_eq!(model.run("run-1").unwrap().permille(), 0);
        assert_eq!(model.selected.as_deref(), Some("run-1"));

        let mut finished = event("run-2", "run.finished");
        finished.state = Some(makepad_widgets::makepad_micro_serde::JsonValue::String(
            "done".into(),
        ));
        model.apply_event(&finished, 3_000);
        assert_eq!(model.run("run-2").unwrap().state, RunState::Done);
        assert_eq!(model.selected.as_deref(), Some("run-1"));
    }

    #[test]
    fn grouping_and_clear_all_preserve_batch_boundaries() {
        let mut model = QueueModel::default();
        model.add_batch("demo", batch(), &[], 7, 1_000);
        let second = CreateBatchResponse {
            batch: "beef".into(),
            runs: vec![BatchRunDto {
                run_id: "run-3".into(),
                instance: "instance-3".into(),
            }],
        };
        model.add_batch("demo", second, &[], 7, 2_000);
        assert_eq!(model.batches.len(), 2);
        assert_eq!(model.items().len(), 5);
        assert!(model.select("run-1"));
        assert_eq!(model.clear_all().batches, vec!["beef", "a3f2"]);
        assert!(model.batches.is_empty());
        assert_eq!(model.selected, None);
    }

    fn play_row(run_id: &str, instance: &str, started_ms: u64) -> RunRowDto {
        RunRowDto {
            run_id: run_id.into(),
            instance: instance.into(),
            flow: "demo".into(),
            batch: None,
            batch_index: None,
            revision: 7,
            state: RunState::Done,
            planned_nodes: Vec::new(),
            nodes: HashMap::new(),
            outputs: HashMap::new(),
            http_log: Vec::new(),
            started_ms,
            finished_ms: Some(started_ms + 1),
        }
    }

    #[test]
    fn a_play_run_lists_without_a_header_and_stays_hidden_once_cleared() {
        let mut model = QueueModel::default();
        model.set_rows(vec![play_row("run-p", "instance-p", 5_000)]);
        assert_eq!(model.items().len(), 1);
        assert!(matches!(model.items()[0], QueueItem::Run(_)));
        assert!(model.batches[0].id.is_none());

        let plan = model.clear_all();
        assert!(plan.batches.is_empty());
        assert_eq!(plan.singles, vec![("run-p".to_string(), "instance-p".to_string())]);
        // The server still lists the run for a while; the snapshot must not
        // bring it back, and it is forgotten once the server drops it.
        model.set_rows(vec![play_row("run-p", "instance-p", 5_000)]);
        assert!(model.items().is_empty());
        model.set_rows(Vec::new());
        assert!(model.hidden.is_empty());
    }

    #[test]
    fn cancelling_one_row_does_not_remove_its_siblings() {
        let mut model = QueueModel::default();
        model.add_batch("demo", batch(), &[], 7, 1_000);
        model.hide_run("run-1");
        assert!(model.run("run-1").is_none());
        assert!(model.run("run-2").is_some());
        assert_eq!(model.batches.len(), 1);
    }

    #[test]
    fn run_row_children_take_precedence_over_selection() {
        assert_eq!(
            resolve_run_row_hit(true, false, true),
            Some(RunRowHit::Cancel)
        );
        assert_eq!(
            resolve_run_row_hit(false, true, true),
            Some(RunRowHit::Asset)
        );
        assert_eq!(
            resolve_run_row_hit(false, false, true),
            Some(RunRowHit::Select)
        );
    }
}
