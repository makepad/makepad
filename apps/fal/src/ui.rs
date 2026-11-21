//! The two lists the window shows: this session's runs (newest first, with
//! their live status, the model's log and the pictures), and the sweep's
//! table (fastest first, filling in as models finish). Both read the app's
//! `State` from the scope while drawing, and hand clicks back to it as
//! `Intent`s.

use crate::engine::{Job, Stage};
use crate::sweep::{cost_per_image, Row};
use crate::State;
use makepad_widgets::*;
use std::path::Path;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    let Meta = Label{
        text: ""
        draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 9}}
    }

    let Thumb = View{
        visible: false
        width: Fit
        height: Fit
        cursor: MouseCursor.Hand
        show_bg: true
        draw_bg +: {color: theme.color_bg_app}
        img := Image{width: 176 height: 176 fit: ImageFit.Smallest}
    }

    mod.widgets.FalRunListBase = #(FalRunList::register_widget(vm))
    mod.widgets.FalRunList = set_type_default() do mod.widgets.FalRunListBase{
        width: Fill
        height: Fill
        flow: Down
        empty := View{
            visible: false
            width: Fill
            height: 160
            align: Align{x: 0.5 y: 0.5}
            hint := Meta{text: ""}
        }
        list := PortalList{
            width: Fill
            height: Fill
            scan: [@Run]
            Run := View{
                width: Fill
                height: Fit
                padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
                card := RoundedView{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 6
                    padding: 12
                    show_bg: true
                    draw_bg +: {color: theme.color_bg_container border_radius: 6.0}
                    head := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 8
                        align: Align{y: 0.5}
                        model := Label{
                            text: ""
                            draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 10}}
                        }
                        status := Meta{}
                        Filler{}
                        copy := ButtonFlat{text: "Copy result JSON"}
                        refetch := ButtonFlat{text: "Fetch again"}
                        cancel := ButtonFlat{text: "Cancel"}
                    }
                    prompt := Label{
                        width: Fill
                        text: ""
                        draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10}}
                    }
                    log := Meta{width: Fill}
                    error := Label{
                        width: Fill
                        text: ""
                        draw_text +: {color: #xe5534b text_style: theme.font_regular{font_size: 10}}
                    }
                    thumbs := View{
                        width: Fill
                        height: Fit
                        flow: Right{wrap: true}
                        spacing: 8
                        s0 := Thumb{}
                        s1 := Thumb{}
                        s2 := Thumb{}
                        s3 := Thumb{}
                    }
                    outputs := Meta{width: Fill}
                }
            }
        }
    }

    mod.widgets.FalSweepTableBase = #(FalSweepTable::register_widget(vm))
    mod.widgets.FalSweepTable = set_type_default() do mod.widgets.FalSweepTableBase{
        width: Fill
        height: Fill
        flow: Down
        header := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 10
            padding: Inset{left: 12 right: 12 top: 6 bottom: 6}
            Meta{width: 32 text: "#"}
            Meta{width: 56 text: ""}
            Meta{width: Fill text: "Model"}
            Meta{width: 84 text: "End to end"}
            Meta{width: 84 text: "Generation"}
            Meta{width: 70 text: "Queue"}
            Meta{width: 110 text: "Price"}
            Meta{width: 200 text: "Status"}
        }
        empty := View{
            visible: false
            width: Fill
            height: 120
            align: Align{x: 0.5 y: 0.5}
            hint := Meta{text: "Load the model list to see what a sweep would run and cost."}
        }
        list := PortalList{
            width: Fill
            height: Fill
            scan: [@Row]
            Row := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 10
                align: Align{y: 0.5}
                padding: Inset{left: 12 right: 12 top: 4 bottom: 4}
                cursor: MouseCursor.Hand
                show_bg: true
                draw_bg +: {color: theme.color_bg_container}
                rank := Label{
                    width: 32
                    text: ""
                    draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 10}}
                }
                thumb := View{
                    width: 56
                    height: 56
                    img := Image{width: 56 height: 56 fit: ImageFit.Smallest}
                }
                names := View{
                    width: Fill
                    height: Fit
                    flow: Down
                    name := Label{
                        width: Fill
                        text: ""
                        draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 10}}
                    }
                    endpoint := Meta{width: Fill}
                }
                e2e := Label{
                    width: 84
                    text: ""
                    draw_text +: {color: theme.color_text text_style: theme.font_bold{font_size: 10}}
                }
                gen := Label{
                    width: 84
                    text: ""
                    draw_text +: {color: theme.color_text text_style: theme.font_regular{font_size: 10}}
                }
                queue := Meta{width: 70}
                price := Meta{width: 110}
                status := Label{
                    width: 200
                    text: ""
                    draw_text +: {color: theme.color_text_meta text_style: theme.font_regular{font_size: 9}}
                }
            }
        }
    }
}

/// What a click in a list asks the app to do.
pub enum Intent {
    /// Show this run's picture large.
    View(usize, usize),
    Cancel(usize),
    CopyResult(usize),
    /// Ask Fal again for a completed run's result.
    Refetch(usize),
}

pub fn seconds(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.2} s"),
        None => "—".to_string(),
    }
}

fn thumb_slot(item: &WidgetRef, cx: &mut Cx, n: usize) -> ViewRef {
    match n {
        0 => item.view(cx, ids!(s0)),
        1 => item.view(cx, ids!(s1)),
        2 => item.view(cx, ids!(s2)),
        _ => item.view(cx, ids!(s3)),
    }
}

/// Show a picture's bytes in an Image (decoded on the task pool; the image
/// cache keeps it by `key`, so redrawing costs nothing).
pub fn show_image(cx: &mut Cx, image: &ImageRef, key: &str, bytes: &std::sync::Arc<[u8]>) {
    let _ = image.load_image_from_data_async(cx, Path::new(key), bytes.clone());
}

#[derive(Script, ScriptHook, Widget)]
pub struct FalRunList {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    /// The job shown at each list position at the last draw.
    #[rust]
    order: Vec<usize>,
}

impl Widget for FalRunList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let list = self.view.portal_list(cx, ids!(list));
            let mut intents = Vec::new();
            for (item_id, item) in list.items_with_actions(actions) {
                let Some(&job) = self.order.get(item_id) else { continue };
                if item.button(cx, ids!(cancel)).clicked(actions) {
                    intents.push(Intent::Cancel(job));
                }
                if item.button(cx, ids!(copy)).clicked(actions) {
                    intents.push(Intent::CopyResult(job));
                }
                if item.button(cx, ids!(refetch)).clicked(actions) {
                    intents.push(Intent::Refetch(job));
                }
                for n in 0..4 {
                    if thumb_slot(&item, cx, n).finger_up(actions).is_some_and(|f| f.is_over) {
                        intents.push(Intent::View(job, n));
                    }
                }
            }
            if let Some(state) = scope.data.get_mut::<State>() {
                state.intents.extend(intents);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // No runs yet: one hint in place of the list, saying what is missing.
        let mut hint = "";
        if let Some(state) = scope.data.get::<State>() {
            self.order = state.single_runs();
            hint = if state.has_key {
                "Your runs show up here: write a prompt and press Generate."
            } else {
                "Your runs show up here: paste your Fal API key above, write a prompt and press Generate."
            };
        }
        let empty = self.order.is_empty();
        self.view.view(cx, ids!(empty)).set_visible(cx, empty);
        self.view.label(cx, ids!(empty.hint)).set_text(cx, hint);
        self.view.portal_list(cx, ids!(list)).set_visible(cx, !empty);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let portal = step.as_portal_list();
            let Some(mut list) = portal.borrow_mut() else { continue };
            let Some(state) = scope.data.get::<State>() else { continue };
            let now = Cx::monotonic_now();
            list.set_item_range(cx, 0, self.order.len());
            while let Some(item_id) = list.next_visible_item(cx) {
                let Some(&index) = self.order.get(item_id) else { continue };
                let job = &state.engine.jobs[index];
                let item = list.item(cx, item_id, id!(Run));
                draw_run(cx, &item, job, now);
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

fn draw_run(cx: &mut Cx2d, item: &WidgetRef, job: &Job, now: f64) {
    item.label(cx, ids!(model)).set_text(cx, &job.endpoint_id);
    item.label(cx, ids!(status)).set_text(cx, &job.status_line(now));
    item.label(cx, ids!(prompt)).set_text(cx, &job.prompt);
    let log = match job.logs.last() {
        Some(last) if job.stage.is_live() => format!("log: {last}"),
        _ if !job.stage.is_live() && job.stage != Stage::Submitting => {
            let mut parts = Vec::new();
            if let Some(t) = job.queue_wait() {
                parts.push(format!("queue {t:.2} s"));
            }
            if let Some(t) = job.generation_time() {
                parts.push(format!("generation {t:.2} s"));
            }
            if let Some(t) = job.end_to_end() {
                parts.push(format!("end to end {t:.2} s"));
            }
            parts.join(" · ")
        }
        _ => String::new(),
    };
    item.label(cx, ids!(log)).set_text(cx, &log);
    let error = match &job.error {
        Some((title, message)) => {
            let mut text = format!("{title}: {message}");
            if job.stage == Stage::Failed && !job.logs.is_empty() {
                text.push_str("\nlog:\n");
                text.push_str(&job.logs.join("\n"));
            }
            text
        }
        None => String::new(),
    };
    item.label(cx, ids!(error)).set_text(cx, &error);
    item.button(cx, ids!(cancel)).set_visible(cx, job.stage.is_live() && job.stage != Stage::Downloading);
    item.button(cx, ids!(copy)).set_visible(cx, job.result.is_some());
    item.button(cx, ids!(refetch)).set_visible(cx, job.refetchable);
    for n in 0..4 {
        let slot = thumb_slot(item, cx, n);
        match job.shots.get(n).and_then(|shot| shot.bytes.as_ref().map(|bytes| (shot, bytes))) {
            Some((shot, bytes)) => {
                slot.set_visible(cx, true);
                let image = slot.image(cx, ids!(img));
                show_image(cx, &image, &shot.key, bytes);
            }
            None => slot.set_visible(cx, false),
        }
    }
    let mut outputs = Vec::new();
    if job.shots.len() > 4 {
        outputs.push(format!("{} more pictures saved beside these.", job.shots.len() - 4));
    }
    for shot in &job.shots {
        if let Some(error) = &shot.error {
            outputs.push(format!("{}: {error}", shot.media.url));
        }
    }
    for media in &job.other_media {
        let kind = media.content_type.clone().unwrap_or_else(|| "file".to_string());
        outputs.push(format!("{kind}: {}", media.url));
    }
    if job.stage == Stage::Done && job.shots.is_empty() && job.other_media.is_empty() {
        outputs.push("No files in the result: use Copy result JSON to see what the model returned.".to_string());
    }
    if let Some(path) = job.shots.first().and_then(|shot| shot.saved.as_ref()) {
        if let Some(dir) = path.parent() {
            outputs.push(format!("Saved in {}", dir.display()));
        }
    }
    item.label(cx, ids!(outputs)).set_text(cx, &outputs.join("\n"));
}

#[derive(Script, ScriptHook, Widget)]
pub struct FalSweepTable {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    order: Vec<usize>,
}

impl Widget for FalSweepTable {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            let list = self.view.portal_list(cx, ids!(list));
            let mut intents = Vec::new();
            for (item_id, item) in list.items_with_actions(actions) {
                let Some(&row) = self.order.get(item_id) else { continue };
                if item.as_view().finger_up(actions).is_some_and(|f| f.is_over) {
                    if let Some(state) = scope.data.get::<State>() {
                        if let Some(job) = state.sweep.rows[row].job {
                            intents.push(Intent::View(job, 0));
                        }
                    }
                }
            }
            if let Some(state) = scope.data.get_mut::<State>() {
                state.intents.extend(intents);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if let Some(state) = scope.data.get::<State>() {
            self.order = state.sweep.order(&state.engine);
        }
        let empty = self.order.is_empty();
        self.view.view(cx, ids!(empty)).set_visible(cx, empty);
        self.view.view(cx, ids!(header)).set_visible(cx, !empty);
        self.view.portal_list(cx, ids!(list)).set_visible(cx, !empty);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let portal = step.as_portal_list();
            let Some(mut list) = portal.borrow_mut() else { continue };
            let Some(state) = scope.data.get::<State>() else { continue };
            let now = Cx::monotonic_now();
            list.set_item_range(cx, 0, self.order.len());
            let mut finished_rank = 0;
            let ranks: Vec<Option<usize>> = self
                .order
                .iter()
                .map(|&row| {
                    let done = state.sweep.rows[row]
                        .job
                        .map(|job| &state.engine.jobs[job])
                        .is_some_and(|job| matches!(job.stage, Stage::Done | Stage::Downloading) && job.end_to_end().is_some());
                    if done {
                        finished_rank += 1;
                        Some(finished_rank)
                    } else {
                        None
                    }
                })
                .collect();
            while let Some(item_id) = list.next_visible_item(cx) {
                let Some(&row) = self.order.get(item_id) else { continue };
                let item = list.item(cx, item_id, id!(Row));
                let job = state.sweep.rows[row].job.map(|job| &state.engine.jobs[job]);
                draw_row(cx, &item, &state.sweep.rows[row], job, ranks[item_id], now);
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

/// The price per picture where it can be compared, else Fal's own unit.
pub fn price_text(row: &Row, job: Option<&Job>) -> String {
    let Some(price) = &row.price else { return "—".to_string() };
    let symbol = if price.currency == "USD" { "$".to_string() } else { format!("{} ", price.currency) };
    match cost_per_image(row, job) {
        Some(cost) => format!("{symbol}{} / image", trim_price(cost)),
        None => format!("{symbol}{} / {}", trim_price(price.unit_price), price.unit),
    }
}

fn trim_price(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0');
    let text = text.strip_suffix('.').unwrap_or(text);
    if text.contains('.') && text.split('.').nth(1).is_some_and(|f| f.len() < 2) {
        format!("{value:.2}")
    } else {
        text.to_string()
    }
}

fn draw_row(cx: &mut Cx2d, item: &WidgetRef, row: &Row, job: Option<&Job>, rank: Option<usize>, now: f64) {
    item.label(cx, ids!(rank)).set_text(cx, &rank.map(|r| r.to_string()).unwrap_or_default());
    item.label(cx, ids!(name)).set_text(cx, &row.display_name);
    item.label(cx, ids!(endpoint)).set_text(cx, &row.endpoint_id);
    item.label(cx, ids!(price)).set_text(cx, &price_text(row, job));
    let shot = job.and_then(|job| job.shots.first()).and_then(|shot| shot.bytes.as_ref().map(|bytes| (shot, bytes)));
    let thumb = item.view(cx, ids!(thumb));
    match shot {
        Some((shot, bytes)) => {
            thumb.set_visible(cx, true);
            let image = thumb.image(cx, ids!(img));
            show_image(cx, &image, &shot.key, bytes);
        }
        None => thumb.set_visible(cx, false),
    }
    let (e2e, gen, queue, status) = match job {
        Some(job) => {
            let status = match &job.error {
                Some((title, message)) => format!("{title}: {message}"),
                None if job.stage.is_live() => job.status_line(now),
                None if row.own_size => "done, at its own size (it refused the sweep's)".to_string(),
                None => "done".to_string(),
            };
            let e2e = match job.end_to_end() {
                Some(t) => format!("{t:.2} s"),
                None if job.stage.is_live() => format!("{:.1} s…", job.elapsed(now)),
                None => "—".to_string(),
            };
            (e2e, seconds(job.generation_time()), seconds(job.queue_wait()), status)
        }
        None => {
            let status = match &row.input {
                Ok(_) => "waiting".to_string(),
                Err(why) => format!("skipped: {why}"),
            };
            ("—".to_string(), "—".to_string(), "—".to_string(), status)
        }
    };
    item.label(cx, ids!(e2e)).set_text(cx, &e2e);
    item.label(cx, ids!(gen)).set_text(cx, &gen);
    item.label(cx, ids!(queue)).set_text(cx, &queue);
    item.label(cx, ids!(status)).set_text(cx, &status);
}
