use {
    crate::{
        makepad_derive_widget::*, makepad_draw::*, view::EventOrder, widget::*,
        widget_tree::CxWidgetExt,
    },
    std::collections::{HashMap, HashSet},
};

const MAX_TRACKS: usize = 4096;
const MAX_CELLS: usize = 65_536;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.AutoFlow = #(AutoFlow::script_api(vm))
    mod.widgets.RepeatMode = #(RepeatMode::script_api(vm))
    mod.widgets.TrackLen = #(TrackLen::script_api(vm))
    mod.widgets.Track = #(Track::script_api(vm))
    mod.widgets.GridBase = set_type_default() do #(Grid::register_widget(vm))
    mod.widgets.Grid = set_type_default() do mod.widgets.GridBase{}
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Script, ScriptHook)]
pub enum AutoFlow {
    #[default]
    #[pick]
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
pub enum RepeatMode {
    #[pick(1)]
    Count(u32),
    AutoFill,
    AutoFit,
}

impl Default for RepeatMode {
    fn default() -> Self {
        Self::Count(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Script)]
pub enum TrackLen {
    #[pick(0.0)]
    Px(f64),
    #[live(0.0)]
    Pct(f64),
    #[live(1.0)]
    Fr(f64),
    #[live(SizeExprId::INVALID)]
    Expr(SizeExprId),
}

impl Default for TrackLen {
    fn default() -> Self {
        Self::Expr(SizeExprId::INVALID)
    }
}

impl ScriptHook for TrackLen {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_string_like() || value.is_object()
    }

    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        if let Some(value) = value.as_number() {
            if valid_nonnegative(value) {
                *self = Self::Px(value);
            } else {
                *self = Self::default();
                grid_diagnostic_once(vm, "Grid track lengths must be finite and nonnegative");
            }
            return true;
        }
        if let Some(source) = script_string(vm, value) {
            match parse_track_len(vm, &source) {
                Ok(parsed) => *self = parsed,
                Err(message) => {
                    *self = Self::default();
                    grid_diagnostic_once(vm, &message);
                }
            }
            return true;
        }
        false
    }
}

#[derive(Clone, Debug, PartialEq, Script)]
pub enum Track {
    #[pick(0.0)]
    Px(f64),
    #[live(0.0)]
    Pct(f64),
    #[live(1.0)]
    Fr(f64),
    #[live(SizeExprId::INVALID)]
    Expr(SizeExprId),
    #[live {
        min: TrackLen::Px(0.0),
        max: TrackLen::Px(0.0),
    }]
    MinMax { min: TrackLen, max: TrackLen },
    #[live {
        mode: RepeatMode::Count(1),
        min: TrackLen::Px(0.0),
        max: TrackLen::Px(0.0),
    }]
    Repeat {
        mode: RepeatMode,
        min: TrackLen,
        max: TrackLen,
    },
}

impl Default for Track {
    fn default() -> Self {
        Self::Expr(SizeExprId::INVALID)
    }
}

impl ScriptHook for Track {
    fn on_type_check(_heap: &ScriptHeap, value: ScriptValue) -> bool {
        value.is_number() || value.is_string_like() || value.is_object()
    }

    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        if let Some(value) = value.as_number() {
            if valid_nonnegative(value) {
                *self = Self::Px(value);
            } else {
                *self = Self::default();
                grid_diagnostic_once(vm, "Grid tracks must be finite and nonnegative");
            }
            return true;
        }
        if let Some(source) = script_string(vm, value) {
            match parse_track(vm, &source) {
                Ok(parsed) => *self = parsed,
                Err(message) => {
                    *self = Self::default();
                    grid_diagnostic_once(vm, &message);
                }
            }
            return true;
        }
        false
    }
}

#[derive(Default)]
struct GridDiagnostics(HashSet<String>);

fn grid_diagnostic_once(vm: &mut ScriptVm, message: &str) {
    if vm
        .cx_mut()
        .global::<GridDiagnostics>()
        .0
        .insert(message.to_string())
    {
        error!("{message}");
    }
}

fn script_string(vm: &mut ScriptVm, value: ScriptValue) -> Option<String> {
    vm.bx
        .heap
        .string_with(value, |_, source| source.to_string())
}

fn valid_nonnegative(value: f64) -> bool {
    value.is_finite() && value >= 0.0
}

fn split_function<'a>(source: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let source = source.trim();
    let body = source.strip_prefix(name)?.trim_start();
    let body = body.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut parts = Vec::new();
    for (index, ch) in body.char_indices() {
        match ch {
            '(' => depth = depth.checked_add(1)?,
            ')' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                parts.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    parts.push(body[start..].trim());
    Some(parts)
}

fn unsupported_intrinsic(source: &str) -> bool {
    matches!(
        source.trim().to_ascii_lowercase().as_str(),
        "auto" | "min-content" | "max-content" | "fit-content"
    )
}

fn parse_number(source: &str, suffix: &str) -> Option<f64> {
    let value = source.trim().strip_suffix(suffix)?.trim().parse().ok()?;
    valid_nonnegative(value).then_some(value)
}

fn parse_track_len(vm: &mut ScriptVm, source: &str) -> Result<TrackLen, String> {
    let source = source.trim();
    if unsupported_intrinsic(source) {
        return Err(format!(
            "Grid intrinsic track length {source:?} is unsupported in the definite release"
        ));
    }
    if let Some(value) = parse_number(source, "fr") {
        return Ok(TrackLen::Fr(value));
    }
    if let Some(value) = parse_number(source, "%") {
        return Ok(TrackLen::Pct(value * 0.01));
    }
    if let Some(value) = parse_number(source, "px") {
        return Ok(TrackLen::Px(value));
    }
    if let Ok(value) = source.parse::<f64>() {
        return valid_nonnegative(value)
            .then_some(TrackLen::Px(value))
            .ok_or_else(|| "Grid track lengths must be finite and nonnegative".to_string());
    }
    let id = vm
        .cx_mut()
        .global::<SizeExprStore>()
        .intern_id(source)
        .map_err(|error| format!("invalid Grid track expression {source:?}: {error}"))?;
    Ok(TrackLen::Expr(id))
}

fn parse_track(vm: &mut ScriptVm, source: &str) -> Result<Track, String> {
    let source = source.trim();
    if let Some(parts) = split_function(source, "minmax") {
        if parts.len() != 2 {
            return Err("Grid minmax() requires exactly two arguments".to_string());
        }
        let min = parse_track_len(vm, parts[0])?;
        if matches!(min, TrackLen::Fr(_)) {
            return Err("Grid minmax() minimum must be definite".to_string());
        }
        return Ok(Track::MinMax {
            min,
            max: parse_track_len(vm, parts[1])?,
        });
    }
    if let Some(parts) = split_function(source, "repeat") {
        if parts.len() != 2 {
            return Err("Grid repeat() requires exactly two arguments".to_string());
        }
        let mode = match parts[0] {
            "auto-fill" => RepeatMode::AutoFill,
            "auto-fit" => RepeatMode::AutoFit,
            count => RepeatMode::Count(
                count
                    .parse::<u32>()
                    .map_err(|_| "Grid repeat count must be a nonnegative integer".to_string())?,
            ),
        };
        let Some(segment) = split_function(parts[1], "minmax") else {
            return Err("Grid repeat() currently requires a minmax() segment".to_string());
        };
        if segment.len() != 2 {
            return Err("Grid repeated minmax() requires exactly two arguments".to_string());
        }
        let min = parse_track_len(vm, segment[0])?;
        if matches!(min, TrackLen::Fr(_)) {
            return Err("Grid repeat() minimum must be definite".to_string());
        }
        return Ok(Track::Repeat {
            mode,
            min,
            max: parse_track_len(vm, segment[1])?,
        });
    }
    Ok(match parse_track_len(vm, source)? {
        TrackLen::Px(value) => Track::Px(value),
        TrackLen::Pct(value) => Track::Pct(value),
        TrackLen::Fr(value) => Track::Fr(value),
        TrackLen::Expr(value) => Track::Expr(value),
    })
}

fn track_len_is_valid(value: TrackLen, store: Option<&SizeExprStore>) -> bool {
    match value {
        TrackLen::Px(value) | TrackLen::Pct(value) | TrackLen::Fr(value) => {
            valid_nonnegative(value)
        }
        TrackLen::Expr(id) => store.and_then(|store| store.source(id)).is_some(),
    }
}

fn track_is_valid(track: &Track, store: Option<&SizeExprStore>) -> bool {
    match track {
        Track::Px(value) | Track::Pct(value) | Track::Fr(value) => valid_nonnegative(*value),
        Track::Expr(id) => store.and_then(|store| store.source(*id)).is_some(),
        Track::MinMax { min, max } => {
            !matches!(min, TrackLen::Fr(_))
                && track_len_is_valid(*min, store)
                && track_len_is_valid(*max, store)
        }
        Track::Repeat { min, max, .. } => {
            !matches!(min, TrackLen::Fr(_))
                && track_len_is_valid(*min, store)
                && track_len_is_valid(*max, store)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Placement {
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
}

impl Placement {
    fn end_col(self) -> usize {
        self.col + self.col_span
    }

    fn end_row(self) -> usize {
        self.row + self.row_span
    }
}

#[derive(Clone, Debug)]
struct ExpandedTrack {
    track: Track,
    auto_fit: bool,
}

#[derive(Clone, Debug, Default)]
struct AxisGeometry {
    lengths: Vec<f64>,
    offsets: Vec<f64>,
    extent: f64,
}

#[derive(Clone)]
enum DrawState {
    Drawing {
        child_index: usize,
        cell_open: bool,
    },
}

#[derive(Script, WidgetRef, WidgetSet, WidgetRegister)]
pub struct Grid {
    #[uid]
    uid: WidgetUid,
    #[source]
    pub source: ScriptObjectRef,
    #[live]
    pub draw_bg: DrawQuad,
    #[live(false)]
    pub show_bg: bool,
    #[layout]
    pub layout: Layout,
    #[walk]
    pub walk: Walk,
    #[live]
    pub columns: Vec<Track>,
    #[live]
    pub rows: Vec<Track>,
    #[live]
    pub column_gap: f64,
    #[live]
    pub row_gap: f64,
    #[live]
    pub auto_flow: AutoFlow,
    #[live]
    pub justify_items: CellAlign,
    #[live]
    pub align_items: CellAlign,
    /// One whitespace-separated row per string; `.` denotes an unnamed cell.
    #[live]
    pub areas: Vec<String>,
    #[live]
    pub implicit_column_size: f64,
    #[live]
    pub implicit_row_size: f64,
    #[live]
    event_order: EventOrder,
    #[live(true)]
    pub visible: bool,

    #[rust]
    area: Area,
    #[rust]
    draw_state: DrawStateWrap<DrawState>,
    #[rust]
    pub children: SmallVec<[(LiveId, WidgetRef); 2]>,
    #[rust]
    live_update_order: SmallVec<[LiveId; 1]>,
    #[rust]
    area_map: HashMap<LiveId, Placement>,
    #[rust]
    expanded_columns: Vec<ExpandedTrack>,
    #[rust]
    expanded_rows: Vec<ExpandedTrack>,
    #[rust]
    column_geometry: AxisGeometry,
    #[rust]
    row_geometry: AxisGeometry,
    #[rust]
    active_columns: Vec<bool>,
    #[rust]
    active_rows: Vec<bool>,
    #[rust]
    column_weights: Vec<f64>,
    #[rust]
    row_weights: Vec<f64>,
    #[rust]
    column_growth_limits: Vec<f64>,
    #[rust]
    row_growth_limits: Vec<f64>,
    #[rust]
    placements: Vec<Option<Placement>>,
    #[rust]
    child_walks: Vec<Walk>,
    #[rust]
    occupancy: HashSet<usize>,
    #[rust]
    prior_columns: Vec<Track>,
    #[rust]
    prior_rows: Vec<Track>,
    #[rust]
    prior_scalars: [f64; 4],
    #[rust]
    indefinite_logged: [bool; 2],
    #[rust]
    placement_cap_logged: bool,
    #[rust]
    occupancy_cap_logged: bool,
    #[rust]
    placement_failure_logged: bool,
}

impl ScriptHook for Grid {
    fn on_before_apply(
        &mut self,
        _vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        if !apply.is_eval() {
            self.live_update_order.clear();
        }
        self.prior_columns.clone_from(&self.columns);
        self.prior_rows.clone_from(&self.rows);
        self.prior_scalars = [
            self.column_gap,
            self.row_gap,
            self.implicit_column_size,
            self.implicit_row_size,
        ];
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(object) = value.as_object() {
                let mut anonymous = 0usize;
                vm.vec_with(object, |vm, values| {
                    for value in values {
                        let id = if let Some(id) = value.key.as_id() {
                            Some(id)
                        } else if value.key.is_nil() {
                            let id = LiveId(anonymous as u64);
                            anonymous += 1;
                            Some(id)
                        } else {
                            None
                        };
                        let Some(id) = id else { continue };
                        if !WidgetRef::value_is_newable_widget(vm, value.value) {
                            continue;
                        }
                        if apply.is_reload() {
                            self.live_update_order.push(id);
                        }
                        if let Some((_, child)) =
                            self.children.iter_mut().find(|(child_id, _)| *child_id == id)
                        {
                            child.script_apply(vm, apply, scope, value.value);
                        } else {
                            self.children.push((
                                id,
                                WidgetRef::script_from_value_scoped(vm, scope, value.value),
                            ));
                        }
                    }
                });
            }
        }
        if apply.is_reload() && (!self.live_update_order.is_empty() || self.children.is_empty()) {
            for (index, id) in self.live_update_order.iter().enumerate() {
                if let Some(position) = self.children.iter().position(|(old, _)| old == id) {
                    self.children.swap(index, position);
                }
            }
            self.children.truncate(self.live_update_order.len());
        }

        let store = vm.cx().get_global_ref::<SizeExprStore>();
        if !self
            .columns
            .iter()
            .all(|track| track_is_valid(track, store))
        {
            self.columns = std::mem::take(&mut self.prior_columns);
            grid_diagnostic_once(vm, "invalid Grid columns; keeping the previous valid value");
        }
        let store = vm.cx().get_global_ref::<SizeExprStore>();
        if !self.rows.iter().all(|track| track_is_valid(track, store)) {
            self.rows = std::mem::take(&mut self.prior_rows);
            grid_diagnostic_once(vm, "invalid Grid rows; keeping the previous valid value");
        }
        let scalar_names = [
            "column_gap",
            "row_gap",
            "implicit_column_size",
            "implicit_row_size",
        ];
        let scalars = [
            &mut self.column_gap,
            &mut self.row_gap,
            &mut self.implicit_column_size,
            &mut self.implicit_row_size,
        ];
        let mut scalar_messages = Vec::new();
        for ((value, previous), name) in scalars
            .into_iter()
            .zip(self.prior_scalars)
            .zip(scalar_names)
        {
            if !valid_nonnegative(*value) {
                *value = previous;
                scalar_messages.push(format!(
                    "invalid Grid {name}; keeping the previous valid value"
                ));
            }
        }
        for message in scalar_messages {
            grid_diagnostic_once(vm, &message);
        }

        let (area_map, diagnostics) = parse_named_areas(&self.areas);
        self.area_map = area_map;
        for diagnostic in diagnostics {
            grid_diagnostic_once(vm, &diagnostic);
        }
        vm.cx_mut().widget_tree_mark_dirty(self.uid);
    }
}

impl WidgetNode for Grid {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn set_scroll_pos(&mut self, cx: &mut Cx, position: Vec2d) {
        self.layout.scroll = position;
        self.redraw(cx);
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
        for (_, child) in &mut self.children {
            child.redraw(cx);
        }
    }

    fn children(&self, visit: &mut dyn FnMut(LiveId, WidgetRef)) {
        for (id, child) in &self.children {
            visit(*id, child.clone());
        }
    }

    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        for (_, child) in &self.children {
            child.find_widgets_from_point(cx, point, found);
        }
    }

    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if visible && matches!(self.area, Area::Empty) {
                cx.redraw_all();
            } else {
                self.redraw(cx);
            }
        }
    }

    fn visible(&self) -> bool {
        self.visible
    }

    fn layer_areas(&self) -> Vec<(&'static str, Area)> {
        self.show_bg
            .then(|| vec![("draw_bg", self.draw_bg.area())])
            .unwrap_or_default()
    }
}

impl Widget for Grid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if !self.visible && event.requires_visibility() {
            return;
        }
        match &self.event_order {
            EventOrder::Up => {
                for (_, child) in self.children.iter_mut().rev() {
                    child.handle_event(cx, event, scope);
                }
            }
            EventOrder::Down => {
                for (_, child) in &mut self.children {
                    child.handle_event(cx, event, scope);
                }
            }
            EventOrder::List(order) => {
                for id in order {
                    if let Some((_, child)) =
                        self.children.iter_mut().find(|(child_id, _)| child_id == id)
                    {
                        child.handle_event(cx, event, scope);
                    }
                }
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.draw_state.begin(
            cx,
            DrawState::Drawing {
                child_index: 0,
                cell_open: false,
            },
        ) {
            if !self.visible {
                self.draw_state.end();
                return DrawStep::done();
            }
            let layout = Layout {
                flow: Flow::Overlay,
                align: Align::default(),
                distribute: Distribute::Start,
                spacing: 0.0,
                wrap_spacing: 0.0,
                ..self.layout
            };
            if self.show_bg {
                self.draw_bg.begin(cx, walk, layout);
            } else {
                cx.begin_turtle(walk, layout);
            }
            self.prepare_layout(cx);
            let origin = cx.turtle().inner_origin();
            cx.walk_turtle(
                Walk::fixed(
                    self.column_geometry.extent,
                    self.row_geometry.extent,
                )
                .with_abs_pos(origin),
            );
        }

        while let Some(DrawState::Drawing {
            child_index,
            cell_open,
        }) = self.draw_state.get()
        {
            if child_index >= self.children.len() {
                if self.show_bg {
                    self.draw_bg.end(cx);
                    self.area = self.draw_bg.area();
                } else {
                    cx.end_turtle_with_area(&mut self.area);
                }
                self.draw_state.end();
                break;
            }
            let Some(placement) = self
                .placements
                .get(child_index)
                .copied()
                .flatten()
            else {
                self.draw_state.set(DrawState::Drawing {
                    child_index: child_index + 1,
                    cell_open: false,
                });
                continue;
            };

            let authored = self.child_walks[child_index];
            if !cell_open {
                let rect = self.cell_rect(cx.turtle().inner_origin(), placement);
                let placement_meta = authored.cell.unwrap_or_default();
                let justify = placement_meta.justify_self.unwrap_or(self.justify_items);
                let align = placement_meta.align_self.unwrap_or(self.align_items);
                cx.begin_turtle(
                    Walk::abs_rect(rect),
                    cell_layout(justify, align),
                );
                self.draw_state.set(DrawState::Drawing {
                    child_index,
                    cell_open: true,
                });
            }

            let placement_meta = authored.cell.unwrap_or_default();
            let child_walk = child_walk_in_cell(
                authored,
                cx.turtle().inner_size(),
                placement_meta.justify_self.unwrap_or(self.justify_items),
                placement_meta.align_self.unwrap_or(self.align_items),
            );
            self.children[child_index]
                .1
                .draw_walk(cx, scope, child_walk)?;
            cx.end_turtle();
            self.draw_state.set(DrawState::Drawing {
                child_index: child_index + 1,
                cell_open: false,
            });
        }
        DrawStep::done()
    }
}

fn cell_align_factor(align: CellAlign) -> f64 {
    match align {
        CellAlign::Stretch | CellAlign::Start => 0.0,
        CellAlign::Center => 0.5,
        CellAlign::End => 1.0,
    }
}

fn cell_layout(justify: CellAlign, align: CellAlign) -> Layout {
    Layout {
        flow: Flow::Overlay,
        align: Align {
            x: cell_align_factor(justify),
            y: cell_align_factor(align),
        },
        clip_x: false,
        clip_y: false,
        ..Default::default()
    }
}

fn child_walk_in_cell(
    mut walk: Walk,
    cell_size: Vec2d,
    justify: CellAlign,
    align: CellAlign,
) -> Walk {
    walk.abs_pos = None;
    if justify == CellAlign::Stretch {
        walk.width = Size::Fixed((cell_size.x - walk.margin.width()).max(0.0));
    }
    if align == CellAlign::Stretch {
        walk.height = Size::Fixed((cell_size.y - walk.margin.height()).max(0.0));
    }
    walk
}

impl Grid {
    fn prepare_layout(&mut self, cx: &mut Cx2d) {
        let inner_size = cx.turtle().inner_size();
        let definite_width = inner_size.x.is_finite().then_some(inner_size.x.max(0.0));
        let definite_height = inner_size.y.is_finite().then_some(inner_size.y.max(0.0));
        expand_tracks(
            &self.columns,
            self.column_gap,
            definite_width,
            |id| eval_grid_expr(cx, id, true, definite_width),
            &mut self.expanded_columns,
        );
        expand_tracks(
            &self.rows,
            self.row_gap,
            definite_height,
            |id| eval_grid_expr(cx, id, false, definite_height),
            &mut self.expanded_rows,
        );

        self.prepare_placements(cx);
        used_tracks_into(
            &self.placements,
            self.expanded_columns.len(),
            true,
            &mut self.active_columns,
        );
        for (active, track) in self.active_columns.iter_mut().zip(&self.expanded_columns) {
            *active = !track.auto_fit || *active;
        }
        used_tracks_into(
            &self.placements,
            self.expanded_rows.len(),
            false,
            &mut self.active_rows,
        );
        for (active, track) in self.active_rows.iter_mut().zip(&self.expanded_rows) {
            *active = !track.auto_fit || *active;
        }
        resolve_axis_into(
            &self.expanded_columns,
            &self.active_columns,
            self.column_gap,
            definite_width,
            |id| eval_grid_expr(cx, id, true, definite_width),
            &mut self.column_geometry,
            &mut self.column_weights,
            &mut self.column_growth_limits,
        );
        resolve_axis_into(
            &self.expanded_rows,
            &self.active_rows,
            self.row_gap,
            definite_height,
            |id| eval_grid_expr(cx, id, false, definite_height),
            &mut self.row_geometry,
            &mut self.row_weights,
            &mut self.row_growth_limits,
        );

        if definite_width.is_none()
            && axis_has_indefinite_share(&self.expanded_columns, |id| {
                cx.size_expr_is_content_independent(id)
            })
            && !self.indefinite_logged[0]
        {
            self.indefinite_logged[0] = true;
            error!("Grid indefinite columns resolve percentage/fr shares to zero and minmax to its definite minimum");
        }
        if definite_height.is_none()
            && axis_has_indefinite_share(&self.expanded_rows, |id| {
                cx.size_expr_is_content_independent(id)
            })
            && !self.indefinite_logged[1]
        {
            self.indefinite_logged[1] = true;
            error!("Grid indefinite rows resolve percentage/fr shares to zero and minmax to its definite minimum");
        }
    }

    fn prepare_placements(&mut self, cx: &mut Cx2d) {
        self.occupancy.clear();
        self.placements.clear();
        let child_limit = self.children.len().min(MAX_CELLS);
        if child_limit < self.children.len() && !self.placement_cap_logged {
            self.placement_cap_logged = true;
            error!("Grid child placement capped at {MAX_CELLS} entries");
        }
        self.placements.resize(child_limit, None);
        self.child_walks.clear();
        self.child_walks.resize(child_limit, Walk::default());

        for (index, (_, child)) in self.children.iter_mut().take(child_limit).enumerate() {
            if child.visible() {
                self.child_walks[index] = child.walk(cx);
            }
        }

        // Named and fully explicit items reserve cells before automatic items,
        // while the final draw still follows source order.
        let mut occupancy_saturated = false;
        for index in 0..child_limit {
            if !self.children[index].1.visible() {
                continue;
            }
            let cell = self.child_walks[index].cell.unwrap_or_default();
            let named = (cell.area != LiveId(0))
                .then(|| self.area_map.get(&cell.area).copied())
                .flatten();
            let explicit = if let Some(named) = named {
                Some(named)
            } else if cell.col != 0 && cell.row != 0 {
                bounded_placement(
                    cell.col as usize - 1,
                    cell.row as usize - 1,
                    normalized_span(cell.col_span),
                    normalized_span(cell.row_span),
                )
            } else {
                None
            };
            if let Some(placement) = explicit {
                self.ensure_implicit_tracks(placement);
                occupancy_saturated |= !occupy(&mut self.occupancy, placement);
                self.placements[index] = Some(placement);
            }
        }

        occupancy_saturated |= self.occupancy.len() >= MAX_CELLS;
        if occupancy_saturated && !self.occupancy_cap_logged {
            self.occupancy_cap_logged = true;
            error!("Grid occupancy capped at {MAX_CELLS} cells; automatic placement stopped");
        }

        for index in 0..child_limit {
            if occupancy_saturated {
                break;
            }
            if !self.children[index].1.visible() || self.placements[index].is_some() {
                continue;
            }
            let cell = self.child_walks[index].cell.unwrap_or_default();
            let col_span = normalized_span(cell.col_span).min(MAX_TRACKS);
            let row_span = normalized_span(cell.row_span).min(MAX_TRACKS);
            let fixed_col = (cell.col != 0).then_some(cell.col as usize - 1);
            let fixed_row = (cell.row != 0).then_some(cell.row as usize - 1);
            let placement = find_auto_placement(
                &self.occupancy,
                self.expanded_columns.len(),
                self.expanded_rows.len(),
                col_span,
                row_span,
                fixed_col,
                fixed_row,
                self.auto_flow,
            );
            if let Some(placement) = placement {
                self.ensure_implicit_tracks(placement);
                if occupy(&mut self.occupancy, placement) {
                    self.placements[index] = Some(placement);
                } else {
                    occupancy_saturated = true;
                    if !self.occupancy_cap_logged {
                        self.occupancy_cap_logged = true;
                        error!("Grid occupancy capped at {MAX_CELLS} cells; automatic placement stopped");
                    }
                }
            } else if !self.placement_failure_logged {
                self.placement_failure_logged = true;
                error!("Grid automatic placement exceeded the definite grid limits; child omitted");
            }
        }
    }

    fn ensure_implicit_tracks(&mut self, placement: Placement) {
        append_implicit(
            &mut self.expanded_columns,
            placement.end_col(),
            self.implicit_column_size,
        );
        append_implicit(
            &mut self.expanded_rows,
            placement.end_row(),
            self.implicit_row_size,
        );
    }

    fn cell_rect(&self, origin: Vec2d, placement: Placement) -> Rect {
        Rect {
            pos: origin
                + dvec2(
                    self.column_geometry.offsets[placement.col],
                    self.row_geometry.offsets[placement.row],
                ),
            size: dvec2(
                span_length(&self.column_geometry, placement.col, placement.col_span),
                span_length(&self.row_geometry, placement.row, placement.row_span),
            ),
        }
    }
}

fn normalized_span(span: u32) -> usize {
    usize::try_from(span.max(1)).unwrap_or(MAX_TRACKS)
}

fn bounded_placement(
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
) -> Option<Placement> {
    if col >= MAX_TRACKS || row >= MAX_TRACKS {
        return None;
    }
    let col_span = col_span.max(1).min(MAX_TRACKS - col);
    let mut row_span = row_span.max(1).min(MAX_TRACKS - row);
    row_span = row_span.min((MAX_CELLS / col_span).max(1));
    Some(Placement {
        col,
        row,
        col_span,
        row_span,
    })
}

fn occupancy_key(col: usize, row: usize) -> usize {
    row * MAX_TRACKS + col
}

fn occupy(occupancy: &mut HashSet<usize>, placement: Placement) -> bool {
    for row in placement.row..placement.end_row() {
        for col in placement.col..placement.end_col() {
            let key = occupancy_key(col, row);
            if !occupancy.contains(&key) && occupancy.len() >= MAX_CELLS {
                return false;
            }
            occupancy.insert(key);
        }
    }
    true
}

fn is_free(occupancy: &HashSet<usize>, placement: Placement) -> bool {
    (placement.row..placement.end_row()).all(|row| {
        (placement.col..placement.end_col())
            .all(|col| !occupancy.contains(&occupancy_key(col, row)))
    })
}

#[allow(clippy::too_many_arguments)]
fn find_auto_placement(
    occupancy: &HashSet<usize>,
    columns: usize,
    rows: usize,
    col_span: usize,
    row_span: usize,
    fixed_col: Option<usize>,
    fixed_row: Option<usize>,
    flow: AutoFlow,
) -> Option<Placement> {
    let col_span = col_span.max(1).min(MAX_TRACKS);
    let row_span = row_span.max(1).min(MAX_TRACKS);
    if col_span.checked_mul(row_span)? > MAX_CELLS {
        return None;
    }
    let mut attempts = 0usize;
    let try_candidate = |col, row| {
        let placement = bounded_placement(col, row, col_span, row_span)?;
        (placement.col_span == col_span
            && placement.row_span == row_span
            && is_free(occupancy, placement))
        .then_some(placement)
    };

    if let Some(col) = fixed_col {
        for row in 0..=MAX_TRACKS - row_span {
            if attempts >= MAX_CELLS {
                break;
            }
            attempts += 1;
            if let Some(placement) = try_candidate(col, row) {
                return Some(placement);
            }
        }
        return None;
    }
    if let Some(row) = fixed_row {
        for col in 0..=MAX_TRACKS - col_span {
            if attempts >= MAX_CELLS {
                break;
            }
            attempts += 1;
            if let Some(placement) = try_candidate(col, row) {
                return Some(placement);
            }
        }
        return None;
    }

    match flow {
        AutoFlow::Row => {
            let columns = columns.max(col_span).min(MAX_TRACKS);
            for row in 0..=MAX_TRACKS - row_span {
                for col in 0..=columns - col_span {
                    if attempts >= MAX_CELLS {
                        return None;
                    }
                    attempts += 1;
                    if let Some(placement) = try_candidate(col, row) {
                        return Some(placement);
                    }
                }
            }
        }
        AutoFlow::Column => {
            let rows = rows.max(row_span).min(MAX_TRACKS);
            for col in 0..=MAX_TRACKS - col_span {
                for row in 0..=rows - row_span {
                    if attempts >= MAX_CELLS {
                        return None;
                    }
                    attempts += 1;
                    if let Some(placement) = try_candidate(col, row) {
                        return Some(placement);
                    }
                }
            }
        }
    }
    None
}

fn append_implicit(tracks: &mut Vec<ExpandedTrack>, required: usize, size: f64) {
    let required = required.min(MAX_TRACKS);
    tracks.reserve(required.saturating_sub(tracks.len()));
    while tracks.len() < required {
        tracks.push(ExpandedTrack {
            track: Track::Px(size),
            auto_fit: false,
        });
    }
}

fn parse_named_areas(rows: &[String]) -> (HashMap<LiveId, Placement>, Vec<String>) {
    let mut diagnostics = Vec::new();
    if rows.is_empty() {
        return (HashMap::new(), diagnostics);
    }
    let cells: Vec<Vec<&str>> = rows
        .iter()
        .map(|row| row.split_whitespace().collect())
        .collect();
    let width = cells[0].len();
    if width == 0 || width > MAX_TRACKS || cells.len() > MAX_TRACKS {
        diagnostics.push("Grid named areas exceed the definite grid limits".to_string());
        return (HashMap::new(), diagnostics);
    }
    if cells.iter().any(|row| row.len() != width) {
        diagnostics.push("Grid named area rows must not be ragged".to_string());
        return (HashMap::new(), diagnostics);
    }

    let mut positions: HashMap<LiveId, Vec<(usize, usize)>> = HashMap::new();
    for (row, values) in cells.iter().enumerate() {
        for (col, value) in values.iter().enumerate() {
            if *value != "." {
                positions
                    .entry(LiveId::from_str(value))
                    .or_default()
                    .push((col, row));
            }
        }
    }
    let mut areas = HashMap::new();
    for (id, positions) in positions {
        let min_col = positions.iter().map(|(col, _)| *col).min().unwrap();
        let max_col = positions.iter().map(|(col, _)| *col).max().unwrap();
        let min_row = positions.iter().map(|(_, row)| *row).min().unwrap();
        let max_row = positions.iter().map(|(_, row)| *row).max().unwrap();
        let col_span = max_col - min_col + 1;
        let row_span = max_row - min_row + 1;
        let rectangular = col_span
            .checked_mul(row_span)
            .is_some_and(|count| count == positions.len())
            && (min_row..=max_row).all(|row| {
                (min_col..=max_col).all(|col| cells[row][col] != "." && LiveId::from_str(cells[row][col]) == id)
            });
        if rectangular {
            areas.insert(
                id,
                Placement {
                    col: min_col,
                    row: min_row,
                    col_span,
                    row_span,
                },
            );
        } else {
            diagnostics.push(format!("Grid named area {id:?} is not rectangular; ignoring it"));
        }
    }
    (areas, diagnostics)
}

fn track_minimum(
    track: &Track,
    definite: Option<f64>,
    eval_expr: &mut impl FnMut(SizeExprId) -> Option<f64>,
) -> f64 {
    match track {
        Track::Px(value) => finite_length(*value),
        Track::Pct(value) => definite.map_or(0.0, |size| finite_length(size * value)),
        Track::Expr(id) => finite_length(eval_expr(*id).unwrap_or(0.0)),
        Track::Fr(_) => 0.0,
        Track::MinMax { min, .. } | Track::Repeat { min, .. } => {
            resolve_track_len(*min, definite, eval_expr).unwrap_or(0.0)
        }
    }
}

fn track_count_size(
    track: &Track,
    definite: Option<f64>,
    eval_expr: &mut impl FnMut(SizeExprId) -> Option<f64>,
) -> f64 {
    match track {
        Track::MinMax { min, max } | Track::Repeat { min, max, .. } => {
            let minimum = resolve_track_len(*min, definite, eval_expr).unwrap_or(0.0);
            resolve_track_len(*max, definite, eval_expr)
                .unwrap_or(minimum)
                .max(minimum)
        }
        _ => track_minimum(track, definite, eval_expr),
    }
}

fn eval_grid_expr(
    cx: &Cx2d,
    id: SizeExprId,
    horizontal: bool,
    definite: Option<f64>,
) -> Option<f64> {
    if definite.is_none() && !cx.size_expr_is_content_independent(id) {
        None
    } else {
        cx.eval_size_expr_in_current_turtle(id, horizontal)
    }
}

fn expanded_once_count(track: &Track) -> usize {
    match track {
        Track::Repeat {
            mode: RepeatMode::Count(count),
            ..
        } => usize::try_from(*count).unwrap_or(MAX_TRACKS).min(MAX_TRACKS),
        _ => 1,
    }
}

fn expand_tracks(
    declared: &[Track],
    gap: f64,
    definite: Option<f64>,
    mut eval_expr: impl FnMut(SizeExprId) -> Option<f64>,
    output: &mut Vec<ExpandedTrack>,
) {
    output.clear();
    let gap = finite_length(gap);
    let mut remaining_count = declared
        .iter()
        .map(expanded_once_count)
        .try_fold(0usize, usize::checked_add)
        .unwrap_or(MAX_TRACKS)
        .min(MAX_TRACKS);
    let mut remaining_measure = declared.iter().fold(0.0, |sum, track| {
        sum + track_count_size(track, definite, &mut eval_expr)
            * expanded_once_count(track) as f64
    });
    let mut emitted_measure = 0.0;

    for track in declared {
        if output.len() >= MAX_TRACKS {
            break;
        }
        let once_count = expanded_once_count(track);
        let once_measure = track_count_size(track, definite, &mut eval_expr);
        let once_total = once_measure * once_count as f64;
        remaining_count = remaining_count.saturating_sub(once_count);
        remaining_measure = (remaining_measure - once_total).max(0.0);
        match track {
            Track::Repeat { mode, min, max } => {
                let repeat_track = Track::MinMax {
                    min: *min,
                    max: *max,
                };
                let minimum = track_minimum(&repeat_track, definite, &mut eval_expr);
                let count_size = track_count_size(&repeat_track, definite, &mut eval_expr);
                let count = match mode {
                    RepeatMode::Count(count) => usize::try_from(*count).unwrap_or(MAX_TRACKS),
                    RepeatMode::AutoFill | RepeatMode::AutoFit => {
                        if let Some(axis) =
                            definite.filter(|_| minimum > 0.0 && count_size > 0.0)
                        {
                            let other_count = output.len().saturating_add(remaining_count);
                            let other_measure = emitted_measure + remaining_measure;
                            let room = if other_count == 0 {
                                axis + gap
                            } else {
                                axis - other_measure - gap * (other_count - 1) as f64
                            };
                            (room / (count_size + gap)).floor().max(1.0) as usize
                        } else {
                            1
                        }
                    }
                }
                .min(MAX_TRACKS - output.len());
                for _ in 0..count {
                    output.push(ExpandedTrack {
                        track: repeat_track.clone(),
                        auto_fit: matches!(mode, RepeatMode::AutoFit),
                    });
                }
                emitted_measure += count_size * count as f64;
            }
            track => {
                output.push(ExpandedTrack {
                    track: track.clone(),
                    auto_fit: false,
                });
                emitted_measure += once_measure;
            }
        }
    }
}

fn finite_length(value: f64) -> f64 {
    if valid_nonnegative(value) { value } else { 0.0 }
}

fn resolve_track_len(
    track: TrackLen,
    definite: Option<f64>,
    eval_expr: &mut impl FnMut(SizeExprId) -> Option<f64>,
) -> Option<f64> {
    match track {
        TrackLen::Px(value) => Some(finite_length(value)),
        TrackLen::Pct(value) => definite.map(|size| finite_length(size * value)),
        TrackLen::Fr(_) => None,
        TrackLen::Expr(id) => eval_expr(id).map(finite_length),
    }
}

fn resolve_axis_into(
    tracks: &[ExpandedTrack],
    active: &[bool],
    gap: f64,
    definite: Option<f64>,
    mut eval_expr: impl FnMut(SizeExprId) -> Option<f64>,
    output: &mut AxisGeometry,
    weights: &mut Vec<f64>,
    growth_limits: &mut Vec<f64>,
) {
    let gap = finite_length(gap);
    let active_count = active.iter().filter(|active| **active).count();
    let gap_total = gap * active_count.saturating_sub(1) as f64;
    output.lengths.clear();
    output.lengths.resize(tracks.len(), 0.0);
    output.offsets.clear();
    output.offsets.resize(tracks.len(), 0.0);
    weights.clear();
    weights.resize(tracks.len(), 0.0);
    growth_limits.clear();
    growth_limits.resize(tracks.len(), 0.0);
    for (index, expanded) in tracks.iter().enumerate() {
        if !active.get(index).copied().unwrap_or(false) {
            continue;
        }
        match &expanded.track {
            Track::Px(value) => output.lengths[index] = finite_length(*value),
            Track::Pct(value) => {
                output.lengths[index] = definite.map_or(0.0, |size| finite_length(size * value))
            }
            Track::Expr(id) => {
                output.lengths[index] = finite_length(eval_expr(*id).unwrap_or(0.0))
            }
            Track::Fr(weight) => weights[index] = finite_length(*weight),
            Track::MinMax { min, max } => {
                output.lengths[index] =
                    resolve_track_len(*min, definite, &mut eval_expr).unwrap_or(0.0);
                if let TrackLen::Fr(weight) = max {
                    weights[index] = finite_length(*weight);
                } else if let Some(maximum) = resolve_track_len(*max, definite, &mut eval_expr) {
                    growth_limits[index] = maximum.max(output.lengths[index]);
                }
            }
            Track::Repeat { .. } => unreachable!("repeat tracks must be expanded"),
        }
    }
    for (limit, length) in growth_limits.iter_mut().zip(&output.lengths) {
        *limit = limit.max(*length);
    }
    if let Some(size) = definite {
        let free = (size - output.lengths.iter().sum::<f64>() - gap_total).max(0.0);
        grow_tracks_to_limits(&mut output.lengths, growth_limits, free);
    }
    let fixed: f64 = output.lengths.iter().sum();
    let weight: f64 = weights.iter().filter(|value| **value > 0.0).sum();
    let remaining = definite
        .map(|size| (size - fixed - gap_total).max(0.0))
        .unwrap_or(0.0);
    if weight.is_finite() && weight > 0.0 && remaining > 0.0 {
        for (length, weight_value) in output.lengths.iter_mut().zip(weights.iter().copied()) {
            if weight_value.is_finite() && weight_value > 0.0 {
                *length += remaining * weight_value / weight;
            }
        }
    }

    let mut cursor = 0.0;
    let mut seen_active = false;
    for index in 0..tracks.len() {
        if active.get(index).copied().unwrap_or(false) {
            if seen_active {
                cursor += gap;
            }
            output.offsets[index] = cursor;
            cursor += output.lengths[index];
            seen_active = true;
        } else {
            output.offsets[index] = cursor;
        }
    }
    output.extent = cursor;
}

#[cfg(test)]
fn resolve_axis(
    tracks: &[ExpandedTrack],
    active: &[bool],
    gap: f64,
    definite: Option<f64>,
    eval_expr: impl FnMut(SizeExprId) -> Option<f64>,
) -> AxisGeometry {
    let mut output = AxisGeometry::default();
    let mut weights = Vec::new();
    let mut growth_limits = Vec::new();
    resolve_axis_into(
        tracks,
        active,
        gap,
        definite,
        eval_expr,
        &mut output,
        &mut weights,
        &mut growth_limits,
    );
    output
}

fn grow_tracks_to_limits(lengths: &mut [f64], limits: &[f64], mut remaining: f64) {
    while remaining > f64::EPSILON {
        let active = lengths
            .iter()
            .zip(limits)
            .filter(|(length, limit)| **limit > **length + f64::EPSILON)
            .count();
        if active == 0 {
            break;
        }
        let share = remaining / active as f64;
        let mut consumed = 0.0;
        for (length, limit) in lengths.iter_mut().zip(limits) {
            if *limit > *length + f64::EPSILON {
                let delta = (*limit - *length).min(share);
                *length += delta;
                consumed += delta;
            }
        }
        if consumed <= f64::EPSILON {
            break;
        }
        remaining = (remaining - consumed).max(0.0);
    }
}

fn used_tracks_into(
    placements: &[Option<Placement>],
    count: usize,
    columns: bool,
    used: &mut Vec<bool>,
) {
    used.clear();
    used.resize(count, false);
    for placement in placements.iter().flatten() {
        let (start, span) = if columns {
            (placement.col, placement.col_span)
        } else {
            (placement.row, placement.row_span)
        };
        for value in start..start.saturating_add(span).min(count) {
            used[value] = true;
        }
    }
}

fn axis_has_indefinite_share(
    tracks: &[ExpandedTrack],
    mut expression_is_content_independent: impl FnMut(SizeExprId) -> bool,
) -> bool {
    tracks.iter().any(|track| match track.track {
        Track::Pct(_) | Track::Fr(_) => true,
        Track::Expr(id) => !expression_is_content_independent(id),
        Track::MinMax { min, max } => {
            matches!(min, TrackLen::Pct(_) | TrackLen::Fr(_))
                || matches!(min, TrackLen::Expr(id) if !expression_is_content_independent(id))
                || matches!(max, TrackLen::Pct(_) | TrackLen::Fr(_))
                || matches!(max, TrackLen::Expr(id) if !expression_is_content_independent(id))
        }
        _ => false,
    })
}

fn span_length(geometry: &AxisGeometry, start: usize, span: usize) -> f64 {
    let last = start + span - 1;
    geometry.offsets[last] + geometry.lengths[last] - geometry.offsets[start]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makepad_script::script;

    fn expanded(tracks: Vec<Track>) -> Vec<ExpandedTrack> {
        tracks
            .into_iter()
            .map(|track| ExpandedTrack {
                track,
                auto_fit: false,
            })
            .collect()
    }

    fn geometry(tracks: Vec<Track>, gap: f64, definite: Option<f64>) -> AxisGeometry {
        let tracks = expanded(tracks);
        resolve_axis(&tracks, &vec![true; tracks.len()], gap, definite, |_| None)
    }

    #[test]
    fn definite_tracks_resolve_fixed_percent_expression_fr_minmax_and_gaps() {
        let expression = SizeExprId(7);
        let tracks = expanded(vec![
            Track::Px(40.0),
            Track::Pct(0.25),
            Track::Expr(expression),
            Track::MinMax {
                min: TrackLen::Px(30.0),
                max: TrackLen::Fr(1.0),
            },
            Track::Fr(2.0),
        ]);
        let result = resolve_axis(&tracks, &[true; 5], 5.0, Some(300.0), |id| {
            (id == expression).then_some(20.0)
        });
        assert_eq!(&result.lengths[..3], &[40.0, 75.0, 20.0]);
        // 115 px remains after fixed minima and four gaps: 1fr + 2fr.
        assert!((result.lengths[3] - (30.0 + 115.0 / 3.0)).abs() < 1e-9);
        assert!((result.lengths[4] - 230.0 / 3.0).abs() < 1e-9);
        assert!((result.extent - 300.0).abs() < 1e-9);
    }

    #[test]
    fn oversubscription_zero_nonfinite_fr_and_minmax_normalization_are_safe() {
        let result = geometry(
            vec![
                Track::Px(90.0),
                Track::Pct(0.5),
                Track::Fr(0.0),
                Track::Fr(f64::NAN),
                Track::MinMax {
                    min: TrackLen::Px(40.0),
                    max: TrackLen::Px(10.0),
                },
            ],
            4.0,
            Some(100.0),
        );
        assert_eq!(result.lengths, vec![90.0, 50.0, 0.0, 0.0, 40.0]);
        assert_eq!(result.extent, 196.0);
        assert!(result.lengths.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn definite_minmax_grows_to_its_limit_before_fr_distribution() {
        let result = geometry(
            vec![
                Track::MinMax {
                    min: TrackLen::Px(10.0),
                    max: TrackLen::Px(50.0),
                },
                Track::Fr(1.0),
            ],
            0.0,
            Some(100.0),
        );
        assert_eq!(result.lengths, vec![50.0, 50.0]);

        let capped = geometry(
            vec![Track::MinMax {
                min: TrackLen::Px(50.0),
                max: TrackLen::Px(100.0),
            }],
            0.0,
            Some(300.0),
        );
        assert_eq!(capped.lengths, vec![100.0]);
        assert_eq!(capped.extent, 100.0);
    }

    #[test]
    fn expression_tracks_use_the_draw_store_without_a_second_evaluator() {
        let mut store = SizeExprStore::default();
        let id = store.intern_id("calc(25% + 10px)").unwrap();
        let result = resolve_axis(
            &expanded(vec![Track::Expr(id)]),
            &[true],
            0.0,
            Some(200.0),
            |id| {
                Some(store.eval(
                    id,
                    SizeExprContext {
                        parent: 200.0,
                        ..Default::default()
                    },
                ))
            },
        );
        assert_eq!(result.lengths, vec![60.0]);
        assert!(store.requires_parent_or_container(id));
        let viewport = store.intern_id("10vw").unwrap();
        assert!(!store.requires_parent_or_container(viewport));
        let tracks = expanded(vec![Track::Expr(id), Track::Expr(viewport)]);
        assert!(axis_has_indefinite_share(&tracks, |id| {
            !store.requires_parent_or_container(id)
        }));
    }

    #[test]
    fn numeric_and_arithmetic_repeats_expand_with_limits() {
        let numeric = vec![Track::Repeat {
            mode: RepeatMode::Count(3),
            min: TrackLen::Px(20.0),
            max: TrackLen::Fr(1.0),
        }];
        let mut output = Vec::new();
        expand_tracks(&numeric, 5.0, Some(100.0), |_| None, &mut output);
        assert_eq!(output.len(), 3);

        let auto = vec![
            Track::Px(20.0),
            Track::Repeat {
                mode: RepeatMode::AutoFill,
                min: TrackLen::Px(30.0),
                max: TrackLen::Fr(1.0),
            },
        ];
        expand_tracks(&auto, 5.0, Some(130.0), |_| None, &mut output);
        assert_eq!(output.len(), 4); // 20 + 3*30 + three 5px gaps = 125

        let capped = vec![Track::Repeat {
            mode: RepeatMode::Count(u32::MAX),
            min: TrackLen::Px(1.0),
            max: TrackLen::Px(1.0),
        }];
        expand_tracks(&capped, 0.0, Some(1.0), |_| None, &mut output);
        assert_eq!(output.len(), MAX_TRACKS);

        let multiple_auto = vec![
            Track::Repeat {
                mode: RepeatMode::AutoFill,
                min: TrackLen::Px(40.0),
                max: TrackLen::Fr(1.0),
            },
            Track::Repeat {
                mode: RepeatMode::AutoFill,
                min: TrackLen::Px(30.0),
                max: TrackLen::Fr(1.0),
            },
        ];
        expand_tracks(&multiple_auto, 5.0, Some(200.0), |_| None, &mut output);
        assert_eq!(output.len(), 5);
        let minimum_extent = output
            .iter()
            .map(|track| track_minimum(&track.track, Some(200.0), &mut |_| None))
            .sum::<f64>()
            + 5.0 * output.len().saturating_sub(1) as f64;
        assert!(minimum_extent <= 200.0);

        let definite_max = vec![Track::Repeat {
            mode: RepeatMode::AutoFill,
            min: TrackLen::Px(30.0),
            max: TrackLen::Px(100.0),
        }];
        expand_tracks(&definite_max, 0.0, Some(300.0), |_| None, &mut output);
        assert_eq!(output.len(), 3);
        assert_eq!(
            resolve_axis(&output, &[true; 3], 0.0, Some(300.0), |_| None).lengths,
            vec![100.0; 3]
        );
    }

    #[test]
    fn auto_fit_collapses_unused_repetitions_without_reindexing_later_tracks() {
        let tracks = vec![
            ExpandedTrack {
                track: Track::Px(20.0),
                auto_fit: false,
            },
            ExpandedTrack {
                track: Track::Px(30.0),
                auto_fit: true,
            },
            ExpandedTrack {
                track: Track::Px(30.0),
                auto_fit: true,
            },
            ExpandedTrack {
                track: Track::Px(40.0),
                auto_fit: false,
            },
        ];
        let result = resolve_axis(
            &tracks,
            &[true, true, false, true],
            5.0,
            Some(200.0),
            |_| None,
        );
        assert_eq!(result.lengths, vec![20.0, 30.0, 0.0, 40.0]);
        assert_eq!(result.offsets, vec![0.0, 25.0, 55.0, 60.0]);
        assert_eq!(result.extent, 100.0);
    }

    #[test]
    fn indefinite_axes_expand_auto_once_and_zero_shares_but_keep_minima() {
        let declared = vec![Track::Repeat {
            mode: RepeatMode::AutoFit,
            min: TrackLen::Px(25.0),
            max: TrackLen::Fr(1.0),
        }];
        let mut output = Vec::new();
        expand_tracks(&declared, 4.0, None, |_| None, &mut output);
        assert_eq!(output.len(), 1);
        let result = geometry(
            vec![
                Track::Pct(0.5),
                Track::Fr(1.0),
                Track::MinMax {
                    min: TrackLen::Px(25.0),
                    max: TrackLen::Fr(1.0),
                },
            ],
            4.0,
            None,
        );
        assert_eq!(result.lengths, vec![0.0, 0.0, 25.0]);
        assert_eq!(result.extent, 33.0);
    }

    #[test]
    fn unsupported_intrinsic_spellings_are_rejected_transactionally() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            assert!(parse_track(vm, "auto").is_err());
            assert!(parse_track(vm, "minmax(min-content, 1fr)").is_err());
            assert!(parse_track(vm, "repeat(auto-fill, 20px)").is_err());
            assert_eq!(parse_track(vm, "25%").unwrap(), Track::Pct(0.25));
            assert_eq!(parse_track(vm, "2fr").unwrap(), Track::Fr(2.0));
        });
    }

    #[test]
    fn grid_and_flattened_cell_metadata_parse_through_the_public_dsl() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut grid = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                Grid{
                    width: 320 height: Fit
                    columns: ["70px", "25%", "1fr", "minmax(20px, 2fr)"]
                    rows: ["repeat(2, minmax(30px, 1fr))"]
                    areas: ["hero hero . ."]
                    first := View{
                        cell: CellPlacement{
                            area: @hero
                            col_span: 2
                            justify_self: CellAlign.Center
                        }
                    }
                    View{}
                }
            });
            Grid::script_from_value(vm, value)
        });
        assert_eq!(grid.columns.len(), 4);
        assert_eq!(grid.rows.len(), 1);
        assert_eq!(grid.children.len(), 2);
        assert_eq!(grid.children[0].0, live_id!(first));
        let child_walk = grid.children[0].1.walk(&mut cx);
        let cell = child_walk.cell.unwrap();
        assert_eq!(cell.area, live_id!(hero));
        assert_eq!(cell.col_span, 2);
        assert_eq!(cell.justify_self, Some(CellAlign::Center));
        assert_eq!(grid.area_map[&live_id!(hero)].col_span, 2);

        cx.with_vm(|vm| {
            let invalid = vm.eval(script! {
                use mod.prelude.widgets.*
                Grid{columns: ["auto"]}
            });
            grid.script_apply(vm, &Apply::Reload, &mut Scope::empty(), invalid);
        });
        assert_eq!(grid.columns.len(), 4);
        assert_eq!(grid.columns[0], Track::Px(70.0));
    }

    #[test]
    fn named_areas_require_nonragged_rectangles() {
        let (areas, diagnostics) = parse_named_areas(&[
            "head head side".into(),
            "main main side".into(),
        ]);
        assert!(diagnostics.is_empty());
        assert_eq!(
            areas[&LiveId::from_str("main")],
            Placement {
                col: 0,
                row: 1,
                col_span: 2,
                row_span: 1,
            }
        );

        let (ragged, diagnostics) =
            parse_named_areas(&["a a".into(), "a".into()]);
        assert!(ragged.is_empty());
        assert_eq!(diagnostics.len(), 1);

        let (non_rectangular, diagnostics) =
            parse_named_areas(&["a a".into(), "a .".into()]);
        assert!(!non_rectangular.contains_key(&LiveId::from_str("a")));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn explicit_overlap_is_allowed_and_auto_placement_avoids_full_spans() {
        let overlap = Placement {
            col: 0,
            row: 0,
            col_span: 2,
            row_span: 2,
        };
        let mut occupancy = HashSet::new();
        occupy(&mut occupancy, overlap);
        occupy(&mut occupancy, overlap);
        assert_eq!(occupancy.len(), 4);

        let placed = find_auto_placement(
            &occupancy,
            3,
            2,
            2,
            1,
            None,
            None,
            AutoFlow::Row,
        )
        .unwrap();
        assert_eq!(placed, Placement { col: 0, row: 2, col_span: 2, row_span: 1 });
    }

    #[test]
    fn row_and_column_auto_flow_are_stable() {
        let mut occupancy = HashSet::new();
        let first = find_auto_placement(
            &occupancy, 2, 2, 1, 1, None, None, AutoFlow::Row,
        )
        .unwrap();
        occupy(&mut occupancy, first);
        let second = find_auto_placement(
            &occupancy, 2, 2, 1, 1, None, None, AutoFlow::Row,
        )
        .unwrap();
        assert_eq!((first.col, first.row), (0, 0));
        assert_eq!((second.col, second.row), (1, 0));

        occupancy.clear();
        occupy(&mut occupancy, first);
        let column = find_auto_placement(
            &occupancy, 2, 2, 1, 1, None, None, AutoFlow::Column,
        )
        .unwrap();
        assert_eq!((column.col, column.row), (0, 1));
    }

    #[test]
    fn implicit_tracks_and_both_allocation_caps_are_bounded() {
        let placement = bounded_placement(MAX_TRACKS - 1, 0, 100, MAX_CELLS).unwrap();
        assert_eq!(placement.col_span, 1);
        assert_eq!(placement.row_span, MAX_TRACKS);
        assert!(placement.col_span * placement.row_span <= MAX_CELLS);
        assert!(bounded_placement(MAX_TRACKS, 0, 1, 1).is_none());

        let mut tracks = Vec::new();
        append_implicit(&mut tracks, usize::MAX, 17.0);
        assert_eq!(tracks.len(), MAX_TRACKS);
        assert!(tracks
            .iter()
            .all(|track| track.track == Track::Px(17.0)));

        let mut occupancy = HashSet::new();
        assert!(!occupy(
            &mut occupancy,
            Placement {
                col: 0,
                row: 0,
                col_span: MAX_TRACKS,
                row_span: MAX_TRACKS,
            },
        ));
        assert_eq!(occupancy.len(), MAX_CELLS);
        assert!(!occupy(
            &mut occupancy,
            Placement {
                col: 1,
                row: 16,
                col_span: 1,
                row_span: 1,
            },
        ));
    }

    #[test]
    fn empty_declared_tracks_and_fit_extent_are_preserved() {
        let result = geometry(
            vec![Track::Px(10.0), Track::Px(20.0), Track::Px(30.0)],
            3.0,
            None,
        );
        assert_eq!(result.lengths.len(), 3);
        assert_eq!(result.extent, 66.0);
        assert_eq!(result.offsets, vec![0.0, 13.0, 36.0]);
    }

    #[test]
    fn cell_alignment_stretch_margins_overrides_and_clipping_match_contract() {
        let authored = Walk {
            abs_pos: Some(dvec2(900.0, 900.0)),
            width: Size::Fixed(25.0),
            height: Size::Fixed(30.0),
            margin: Inset {
                left: 2.0,
                right: 3.0,
                top: 4.0,
                bottom: 6.0,
            },
            ..Default::default()
        };
        let stretched = child_walk_in_cell(
            authored,
            dvec2(100.0, 80.0),
            CellAlign::Stretch,
            CellAlign::Stretch,
        );
        assert_eq!(stretched.abs_pos, None);
        assert_eq!(stretched.width, Size::Fixed(95.0));
        assert_eq!(stretched.height, Size::Fixed(70.0));

        for (align, factor) in [
            (CellAlign::Start, 0.0),
            (CellAlign::Center, 0.5),
            (CellAlign::End, 1.0),
        ] {
            let preserved = child_walk_in_cell(authored, dvec2(100.0, 80.0), align, align);
            assert_eq!(preserved.width, authored.width);
            assert_eq!(preserved.height, authored.height);
            assert_eq!(cell_layout(align, align).align.x, factor);
            assert_eq!(cell_layout(align, align).align.y, factor);
        }
        let cell = cell_layout(CellAlign::Center, CellAlign::End);
        assert!(!cell.clip_x && !cell.clip_y);
        assert_eq!(cell.flow, Flow::Overlay);
    }

    #[test]
    fn spans_include_internal_gaps_and_auto_fit_collapsed_tracks() {
        let geometry = AxisGeometry {
            lengths: vec![20.0, 0.0, 30.0],
            offsets: vec![0.0, 25.0, 25.0],
            extent: 55.0,
        };
        assert_eq!(span_length(&geometry, 0, 3), 55.0);
        assert_eq!(span_length(&geometry, 2, 1), 30.0);
    }

    #[test]
    fn oversized_child_is_not_cell_clipped_and_only_obeys_the_grid_clip() {
        use crate::makepad_draw::cx_draw::CxDraw;

        fn make_grid(cx: &mut Cx, clip: bool) -> Grid {
            cx.with_vm(|vm| {
                let source = script! {
                    use mod.prelude.widgets.*
                    Grid{
                        width: 100 height: 40
                        clip_x: #(clip) clip_y: false
                        justify_items: CellAlign.Start
                        columns: ["50px"] rows: ["40px"]
                        child := View{width: 160 height: 20}
                    }
                };
                let value = vm.eval(source);
                Grid::script_from_value(vm, value)
            })
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut clipped = make_grid(&mut cx, true);
        let mut unclipped = make_grid(&mut cx, false);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, dvec2(300.0, 100.0));
        let mut draw_list = DrawList2d::new(&mut cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(300.0, 100.0), Layout::flow_overlay());
        assert!(clipped
            .draw_walk(&mut cx2d, &mut Scope::empty(), clipped.walk)
            .is_done());
        let clipped_area = clipped.children[0].1.area();
        assert!(unclipped
            .draw_walk(
                &mut cx2d,
                &mut Scope::empty(),
                unclipped.walk.with_abs_pos(dvec2(120.0, 0.0)),
            )
            .is_done());
        let unclipped_area = unclipped.children[0].1.area();
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
        drop(cx2d);
        drop(draw);

        assert_eq!(clipped_area.rect(&cx).size.x, 160.0);
        assert_eq!(unclipped_area.rect(&cx).size.x, 160.0);
        let clipped_values = match clipped_area {
            Area::Rect(area) => {
                let value = &cx.draw_lists[area.draw_list_id].rect_areas[area.rect_id];
                (value.rect, value.draw_clip)
            }
            _ => panic!("expected a rect area"),
        };
        let unclipped_values = match unclipped_area {
            Area::Rect(area) => {
                let value = &cx.draw_lists[area.draw_list_id].rect_areas[area.rect_id];
                (value.rect, value.draw_clip)
            }
            _ => panic!("expected a rect area"),
        };
        let raw_clipped = clipped_values.0.clip(clipped_values.1);
        let raw_unclipped = unclipped_values.0.clip(unclipped_values.1);
        assert_eq!(raw_clipped.size.x, 100.0);
        assert_eq!(raw_unclipped.size.x, 160.0);
    }

    #[test]
    fn yielding_child_resumes_in_the_same_cell_and_nested_grid_draws() {
        use crate::makepad_draw::cx_draw::CxDraw;

        fn assert_widget<T: Widget>() {}
        assert_widget::<Grid>();

        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut grid = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                Grid{
                    columns: ["80px", "1fr"]
                    rows: ["40px"]
                    hidden := View{
                        visible: false
                        cell: CellPlacement{col: 1 row: 1}
                    }
                    step := TurtleStep{cell: CellPlacement{col: 1 row: 1}}
                    nested := Grid{
                        cell: CellPlacement{col: 2 row: 1}
                        columns: ["20px"] rows: ["20px"]
                        View{}
                    }
                }
            });
            Grid::script_from_value(vm, value)
        });
        let pass = DrawPass::new(&mut cx);
        let mut draw_list = DrawList2d::new(&mut cx);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(&mut cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.begin_pass(&pass, None);
        draw_list.begin_always(&mut cx2d);
        cx2d.begin_root_turtle(dvec2(200.0, 100.0), Layout::default());
        assert!(grid
            .draw_walk(
                &mut cx2d,
                &mut Scope::empty(),
                Walk::fixed(200.0, 100.0),
            )
            .is_step());
        assert_eq!(grid.placements[0], None);
        let Some(DrawState::Drawing {
            child_index,
            cell_open,
        }) = grid.draw_state.get()
        else {
            panic!("Grid lost its yielding draw state")
        };
        assert_eq!(child_index, 1);
        assert!(cell_open);
        assert!(grid
            .draw_walk(
                &mut cx2d,
                &mut Scope::empty(),
                Walk::fixed(200.0, 100.0),
            )
            .is_done());
        assert!(grid.draw_state.get().is_none());
        cx2d.end_pass_sized_turtle();
        draw_list.end(&mut cx2d);
        cx2d.end_pass(&pass);
    }

    #[test]
    fn walk_and_layout_size_gates_and_cell_default_remain_stable() {
        assert!(std::mem::size_of::<Walk>() <= 384);
        assert!(std::mem::size_of::<Layout>() <= 112);
        assert_eq!(Walk::default().cell, None);
    }
}
