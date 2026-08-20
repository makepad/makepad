//! DataGrid + charts: a live market table with Sparkline cells, wired to a
//! pannable/zoomable line chart and candlestick chart for the selected row.
//! Prices tick a few times per second; only visible cells are drawn.

use makepad_widgets::*;

const SYMBOLS: [&str; 28] = [
    "MKPD", "RUST", "QUAD", "GLYF", "SDF", "TRTL", "WGPU", "METL", "VULK", "SHDR", "CELL", "GRID",
    "VIRT", "SCRL", "DOCK", "SPLT", "FOLD", "PORT", "FLAT", "TREE", "ANIM", "EASE", "BEZR", "PIXL",
    "TEXR", "ATLS", "FONT", "SLUG",
];

const HISTORY: usize = 240;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.ChartsTabBase = #(ChartsTab::register_widget(vm))
    mod.widgets.ChartsTab = set_type_default() do mod.widgets.ChartsTabBase{
        width: Fill height: Fill
        flow: Down

        grid := DataGrid{
            width: Fill height: Fill
            rows: 28
            cols: 7
            zebra_stripes: true
            default_row_height: 30.0
            row_header_width: 40.0

            Trend := View{
                width: Fill height: Fill
                padding: Inset{left: 4, right: 4, top: 5, bottom: 5}
                spark := Sparkline{}
            }
        }

        charts_header := View{
            width: Fill height: 30
            flow: Right spacing: 10
            padding: Inset{left: 10, right: 10, top: 6, bottom: 4}
            align: Align{y: 0.5}
            show_bg: true
            draw_bg +: {color: #x14142a}

            chart_title := Label{
                text: "MKPD — live"
                draw_text +: {color: #xd0d4e8 text_style: theme.font_bold{font_size: 10.0}}
            }
            Label{
                text: "click a row to chart it · prices tick live 4×/s"
                draw_text +: {color: #x666a88 text_style +: {font_size: 8.5}}
            }
        }

        charts_pane := View{
            width: Fill height: 320
            flow: Right

            line := TrendChart{
                width: Fill height: Fill
            }
            candles := TrendChart{
                width: 470 height: Fill
                color_line: #x66bb6a
            }
        }
    }
}

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

struct Market {
    /// price history per symbol, most recent last
    history: Vec<Vec<f64>>,
    volumes: Vec<f64>,
    step: u64,
}

impl Market {
    fn new() -> Self {
        let mut history = Vec::new();
        let mut volumes = Vec::new();
        for (i, _) in SYMBOLS.iter().enumerate() {
            let base = 20.0 + (mix64(i as u64 * 7 + 1) % 40000) as f64 / 100.0;
            let mut prices = Vec::with_capacity(HISTORY);
            let mut p = base;
            for t in 0..HISTORY {
                let r = mix64((i as u64) << 32 | t as u64) as f64 / u64::MAX as f64;
                p *= 1.0 + (r - 0.5) * 0.012;
                prices.push(p);
            }
            volumes.push((mix64(i as u64 * 31 + 7) % 9_000_000) as f64 + 500_000.0);
            history.push(prices);
        }
        Self {
            history,
            volumes,
            step: HISTORY as u64,
        }
    }

    fn tick(&mut self) {
        self.step += 1;
        for (i, prices) in self.history.iter_mut().enumerate() {
            let r = mix64((i as u64) << 32 | self.step) as f64 / u64::MAX as f64;
            let last = *prices.last().unwrap();
            let next = last * (1.0 + (r - 0.5) * 0.012);
            prices.push(next);
            if prices.len() > HISTORY {
                prices.remove(0);
            }
            let rv = mix64((i as u64) << 40 | self.step) % 40000;
            self.volumes[i] += rv as f64 - 19000.0;
            self.volumes[i] = self.volumes[i].max(100_000.0);
        }
    }

    fn last(&self, i: usize) -> f64 {
        *self.history[i].last().unwrap()
    }

    fn change(&self, i: usize) -> f64 {
        let h = &self.history[i];
        h[h.len() - 1] - h[0]
    }

    fn day_range(&self, i: usize) -> (f64, f64) {
        let h = &self.history[i];
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for v in h {
            min = min.min(*v);
            max = max.max(*v);
        }
        (min, max)
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ChartsTab {
    #[deref]
    view: View,
    #[rust(Market::new())]
    market: Market,
    #[rust]
    selected: usize,
    #[rust]
    timer: Option<Timer>,
    #[rust]
    initialized: bool,
}

impl ChartsTab {
    fn feed_charts(&mut self, cx: &mut Cx) {
        let prices = &self.market.history[self.selected];
        self.view.trend_chart(cx, ids!(line)).set_series(cx, prices);
        // bucket the price history into candles
        let bucket = 8;
        let mut candles = Vec::new();
        let mut i = 0;
        while i + bucket <= prices.len() {
            let slice = &prices[i..i + bucket];
            let mut high = f64::NEG_INFINITY;
            let mut low = f64::INFINITY;
            for v in slice {
                high = high.max(*v);
                low = low.min(*v);
            }
            candles.push(Candle {
                time: (i / bucket) as f64,
                open: slice[0],
                high,
                low,
                close: slice[bucket - 1],
                volume: 1.0,
            });
            i += bucket;
        }
        self.view
            .trend_chart(cx, ids!(candles))
            .set_candles(cx, candles);
        self.view.label(cx, ids!(chart_title)).set_text(
            cx,
            &format!(
                "{} — {:.2}  ({:+.2}%)",
                SYMBOLS[self.selected],
                self.market.last(self.selected),
                self.market.change(self.selected) / self.market.history[self.selected][0] * 100.0
            ),
        );
    }
}

impl Widget for ChartsTab {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.timer.is_none() {
            self.timer = Some(cx.start_interval(0.25));
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut grid) = step.as_data_grid().borrow_mut() {
                if !self.initialized {
                    self.initialized = true;
                    grid.set_col_labels(
                        ["Symbol", "Last", "Δ", "Δ%", "Trend", "Range", "Volume"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    );
                    grid.set_col_width(0, 80.0);
                    grid.set_col_width(1, 90.0);
                    grid.set_col_width(2, 80.0);
                    grid.set_col_width(3, 80.0);
                    grid.set_col_width(4, 240.0);
                    grid.set_col_width(5, 150.0);
                    grid.set_col_width(6, 110.0);
                }
                grid.set_grid_size(SYMBOLS.len(), 7);
                while let Some(cell) = grid.next_cell(cx) {
                    let i = cell.row;
                    let chg = self.market.change(i);
                    let up = chg >= 0.0;
                    let chg_color = if up {
                        Some(vec4(0.09, 0.55, 0.35, 1.0))
                    } else {
                        Some(vec4(0.82, 0.22, 0.2, 1.0))
                    };
                    match cell.col {
                        0 => grid.cell_text_styled(
                            cx,
                            &cell,
                            SYMBOLS[i],
                            CellStyle {
                                bold: true,
                                ..CellStyle::default()
                            },
                        ),
                        1 => grid.cell_text_styled(
                            cx,
                            &cell,
                            &format!("{:.2}", self.market.last(i)),
                            CellStyle {
                                align: 1.0,
                                ..CellStyle::default()
                            },
                        ),
                        2 => grid.cell_text_styled(
                            cx,
                            &cell,
                            &format!("{:+.2}", chg),
                            CellStyle {
                                align: 1.0,
                                color: chg_color,
                                ..CellStyle::default()
                            },
                        ),
                        3 => {
                            let pct = chg / self.market.history[i][0] * 100.0;
                            grid.cell_text_styled(
                                cx,
                                &cell,
                                &format!("{:+.2}%", pct),
                                CellStyle {
                                    align: 1.0,
                                    color: chg_color,
                                    bold: true,
                                    ..CellStyle::default()
                                },
                            )
                        }
                        4 => {
                            if let Some(item) = grid.item(cx, cell.row, cell.col, id!(Trend)) {
                                let h = &self.market.history[i];
                                let tail = &h[h.len().saturating_sub(80)..];
                                item.sparkline(cx, ids!(spark)).set_values(cx, tail);
                                grid.draw_item(cx, &cell, &item, None);
                            }
                        }
                        5 => {
                            let (min, max) = self.market.day_range(i);
                            grid.cell_text_styled(
                                cx,
                                &cell,
                                &format!("{:.1} – {:.1}", min, max),
                                CellStyle {
                                    align: 0.5,
                                    color: Some(vec4(0.45, 0.45, 0.5, 1.0)),
                                    ..CellStyle::default()
                                },
                            )
                        }
                        _ => grid.cell_text_styled(
                            cx,
                            &cell,
                            &format!("{:.0}", self.market.volumes[i]),
                            CellStyle {
                                align: 1.0,
                                ..CellStyle::default()
                            },
                        ),
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);

        if let Some(timer) = &self.timer {
            if timer.is_event(event).is_some() {
                self.market.tick();
                self.feed_charts(cx);
                self.view.data_grid(cx, ids!(grid)).redraw(cx);
            }
        }

        let Event::Actions(actions) = event else {
            return;
        };
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            if let DataGridAction::SelectionChanged {
                selection: Some(sel),
            } = action
            {
                if sel.head.0 != self.selected {
                    self.selected = sel.head.0;
                    self.feed_charts(cx);
                }
            }
        }
    }
}
