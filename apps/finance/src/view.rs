//! The app: one window that is a desktop app when it is wide and a phone
//! app when it is narrow.
//!
//! The same screens, the same data, one code path — what changes with
//! width is the CHROME (a sidebar becomes a bottom tab bar) and the
//! DENSITY (a nine-column register becomes three columns of taller rows).
//! Nothing is duplicated per form factor, because two implementations of
//! one screen drift apart within a week.
//!
//! The register is a `DataGrid`, which draws only the cells inside the
//! viewport and puts a whole screen of them in two draw calls. That is the
//! reason this app can hold a decade of transactions in one list and still
//! scroll at frame rate, which is exactly where the products it is
//! measured against fall over.

use crate::chart::{FinanceChartWidgetExt, MeterWidgetRefExt};
use crate::date::{self, DateRange, Day, MonthKey};
use crate::model::*;
use crate::money::{format_compact, format_minor, format_money, Currency};
use crate::report;
use crate::runtime::{Backend, ImportState, Runtime};
use crate::theme;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.FinanceBase = #(Finance::register_widget(vm))

    let Panel = SolidView{
        draw_bg +: { color: mod.finance.panel }
    }

    let Card = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        padding: 16
        spacing: 8
        draw_bg +: {
            color: mod.finance.panel
            border_radius: 10.0
            border_size: 1.0
            border_color: mod.finance.line_soft
        }
    }

    let Title = Label{
        draw_text +: {
            color: mod.finance.fg
            text_style: theme.font_bold{font_size: 12}
        }
    }

    let Body = Label{
        draw_text +: {
            color: mod.finance.fg
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    let Dim = Label{
        draw_text +: {
            color: mod.finance.fg_dim
            text_style: theme.font_regular{font_size: 8.5}
        }
    }

    let Money = Label{
        draw_text +: {
            color: mod.finance.fg
            text_style: theme.font_code{font_size: 10}
        }
    }

    let Big = Label{
        draw_text +: {
            color: mod.finance.fg
            text_style: theme.font_bold{font_size: 26}
        }
    }

    // A flat button that reads as a row, not a control: the whole nav is
    // made of these and a stack of bevels would be noise.
    let NavItem = Button{
        width: Fill
        height: 34
        align: Align{x: 0.0, y: 0.5}
        padding: Inset{left: 12, right: 10, top: 6, bottom: 6}
        draw_bg +: {
            color: #x00000000
            color_hover: mod.finance.raised
            color_down: mod.finance.accent_soft
            color_focus: #x00000000
            border_radius: 6.0
            border_size: 0.0
        }
        draw_text +: {
            color: mod.finance.fg_dim
            color_hover: mod.finance.fg
            color_down: mod.finance.fg
            color_focus: mod.finance.fg_dim
            text_style: theme.font_regular{font_size: 10}
        }
    }

    let Chip = Button{
        height: 26
        padding: Inset{left: 12, right: 12, top: 4, bottom: 4}
        draw_bg +: {
            color: mod.finance.raised
            color_hover: mod.finance.line
            color_down: mod.finance.accent_soft
            color_focus: mod.finance.raised
            border_radius: 13.0
            border_size: 0.0
        }
        draw_text +: {
            color: mod.finance.fg_dim
            color_hover: mod.finance.fg
            color_down: mod.finance.fg
            color_focus: mod.finance.fg_dim
            text_style: theme.font_regular{font_size: 9}
        }
    }

    let Primary = Button{
        height: 30
        padding: Inset{left: 16, right: 16, top: 6, bottom: 6}
        draw_bg +: {
            color: mod.finance.accent
            color_hover: #x5d99ff
            color_down: #x3b7ae6
            color_focus: mod.finance.accent
            border_radius: 6.0
            border_size: 0.0
        }
        draw_text +: {
            color: #xffffff
            color_hover: #xffffff
            color_down: #xffffff
            color_focus: #xffffff
            text_style: theme.font_bold{font_size: 9.5}
        }
    }

    let Search = TextInput{
        height: 28
        empty_text: "Search payee, memo, amount…"
        draw_text +: {
            color: mod.finance.fg
            color_hover: mod.finance.fg
            color_focus: mod.finance.fg
            color_down: mod.finance.fg
            color_empty: mod.finance.fg_faint
            color_empty_hover: mod.finance.fg_dim
            color_empty_focus: mod.finance.fg_faint
            text_style: theme.font_regular{font_size: 9.5}
        }
    }

    // One row of the account list: name, kind, balance.
    let AccountRow = View{
        width: Fill
        height: Fit
        flow: Down
        padding: Inset{left: 12, right: 10, top: 7, bottom: 7}
        spacing: 2
        flow: Overlay
        acc_hit := Button{
            width: Fill
            height: Fill
            text: ""
            draw_bg +: {
                color: #x00000000
                color_hover: mod.finance.raised
                color_down: mod.finance.accent_soft
                color_focus: #x00000000
                border_radius: 6.0
                border_size: 0.0
            }
        }
        acc_rows := View{
            width: Fill
            height: Fit
            flow: Down
            spacing: 2
            acc_top := View{
                width: Fill
                height: Fit
                flow: Right
                acc_name := Body{ width: Fill }
                acc_balance := Money{ width: Fit }
            }
            acc_kind := Dim{}
        }
    }

    let Ledger = DataGrid{
        width: Fill
        height: Fill
        rows: 0
        cols: 7
        default_col_width: 130.0
        default_row_height: 26.0
        col_header_height: 26.0
        row_header_width: 0.0
        cell_pad_x: 10.0
        zebra_stripes: true
        // The stock DataGrid is a light-mode spreadsheet; every surface it
        // paints has to be restated or the register turns up white.
        color_bg: mod.finance.bg
        color_cell: mod.finance.bg
        color_cell_alt: mod.finance.zebra
        color_text: mod.finance.fg
        color_header: mod.finance.panel
        color_header_active: mod.finance.raised
        color_header_text: mod.finance.fg_dim
        color_selection: mod.finance.select
        color_selection_border: mod.finance.accent
        color_drag_marker: mod.finance.accent
        color_resize_guide: mod.finance.accent_soft
        // The stock scrollbar handle is translucent BLACK, which is
        // invisible on a dark surface.
        scroll_bar_h: mod.widgets.ScrollBar{
            draw_bg +: {
                color: uniform(#xffffff26)
                color_hover: uniform(#xffffff42)
                color_drag: uniform(#xffffff66)
            }
        }
        scroll_bar_v: mod.widgets.ScrollBar{
            draw_bg +: {
                color: uniform(#xffffff26)
                color_hover: uniform(#xffffff42)
                color_drag: uniform(#xffffff66)
            }
        }
        draw_cell +: {
            border_color: uniform(mod.finance.line_soft)
            border_size: uniform(1.0)
        }
        draw_text +: {
            color: mod.finance.fg
            text_style: theme.font_code{font_size: 9}
        }
        draw_text_bold +: {
            color: mod.finance.fg
            text_style: theme.font_code{font_size: 9}
        }
    }

    // A bar in a "where the money went" list: label, track, value.
    let BarRow = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: 4
        margin: Inset{top: 4, bottom: 4}
        bar_head := View{
            width: Fill
            height: Fit
            flow: Right
            bar_label := Body{ width: Fill }
            bar_value := Label{
                width: Fit
                draw_text +: {
                    color: mod.finance.fg_dim
                    text_style: theme.font_code{font_size: 9}
                }
            }
        }
        bar_meter := Meter{}
    }

    let StatCard = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        padding: 14
        spacing: 6
        draw_bg +: {
            color: mod.finance.panel
            border_radius: 10.0
            border_size: 1.0
            border_color: mod.finance.line_soft
        }
        stat_label := Dim{}
        stat_value := Label{
            draw_text +: {
                color: mod.finance.fg
                text_style: theme.font_code{font_size: 15}
            }
        }
        stat_note := Dim{}
    }

    let Trend = FinanceChart{
        width: Fill
        height: Fill
        color_line: mod.finance.c0
        color_fill: mod.finance.c0
        color_second: mod.finance.c1
        color_axis: mod.finance.fg_faint
        color_rule: mod.finance.line
    }

    mod.widgets.Finance = set_type_default() do mod.widgets.FinanceBase{
        width: Fill
        height: Fill
        flow: Down
        show_bg: true
        draw_bg +: { color: mod.finance.bg }

        body := View{
            width: Fill
            height: Fill
            flow: Right

            // ---- Sidebar: navigation and the account list. Hidden when
            // the window is too narrow to spare 240 points for it.
            sidebar := Panel{
                width: 248
                height: Fill
                flow: Down
                padding: Inset{left: 10, right: 10, top: 14, bottom: 10}
                spacing: 4

                brand := Label{
                    margin: Inset{left: 10, bottom: 10}
                    draw_text +: {
                        color: mod.finance.fg
                        text_style: theme.font_bold{font_size: 14}
                    }
                    text: "Finance"
                }

                nav_overview := NavItem{ text: "Overview" }
                nav_ledger := NavItem{ text: "Transactions" }
                nav_budget := NavItem{ text: "Budget" }
                nav_reports := NavItem{ text: "Reports" }
                nav_import := NavItem{ text: "Import" }

                Hr{ height: 18 }

                net_worth_label := Dim{
                    margin: Inset{left: 12}
                    text: "Net worth"
                }
                net_worth_value := Label{
                    margin: Inset{left: 12, bottom: 8}
                    draw_text +: {
                        color: mod.finance.fg
                        text_style: theme.font_code{font_size: 17}
                    }
                }

                accounts_label := Dim{
                    margin: Inset{left: 12, top: 4, bottom: 2}
                    text: "ACCOUNTS"
                }
                accounts_list := PortalList{
                    width: Fill
                    height: Fill
                    Account := AccountRow{}
                }
            }

            content := View{
                width: Fill
                height: Fill
                flow: Down

                // ---- Top bar: title, search, and the range chips.
                topbar := Panel{
                    width: Fill
                    height: 52
                    flow: Right
                    align: Align{x: 0.0, y: 0.5}
                    padding: Inset{left: 16, right: 16}
                    spacing: 10
                    screen_title := Title{ width: Fit }
                    search_input := Search{ width: Fill }
                    range_month := Chip{ text: "Month" }
                    range_quarter := Chip{ text: "90 days" }
                    range_year := Chip{ text: "Year" }
                    range_all := Chip{ text: "All" }
                }

                screens := View{
                    width: Fill
                    height: Fill
                    flow: Overlay

                    // ================= OVERVIEW =================
                    overview := ScrollYView{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 16
                        spacing: 14

                        stats_row := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 12
                            stat_in := StatCard{}
                            stat_out := StatCard{}
                            stat_net := StatCard{}
                            stat_saved := StatCard{}
                        }

                        worth_card := Card{
                            height: 260
                            worth_title := Title{ text: "Net worth" }
                            worth_sub := Dim{}
                            worth_chart := Trend{}
                        }

                        lower_row := View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 12

                            spend_card := Card{
                                width: Fill
                                spend_title := Title{ text: "Where it went" }
                                cat_0 := BarRow{}
                                cat_1 := BarRow{}
                                cat_2 := BarRow{}
                                cat_3 := BarRow{}
                                cat_4 := BarRow{}
                                cat_5 := BarRow{}
                                cat_6 := BarRow{}
                                cat_7 := BarRow{}
                            }

                            upcoming_card := Card{
                                width: Fill
                                upcoming_title := Title{ text: "Coming up" }
                                due_0 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_0 := Body{ width: Fill }
                                    due_amount_0 := Body{ width: Fit }
                                }
                                due_1 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_1 := Body{ width: Fill }
                                    due_amount_1 := Body{ width: Fit }
                                }
                                due_2 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_2 := Body{ width: Fill }
                                    due_amount_2 := Body{ width: Fit }
                                }
                                due_3 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_3 := Body{ width: Fill }
                                    due_amount_3 := Body{ width: Fit }
                                }
                                due_4 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_4 := Body{ width: Fill }
                                    due_amount_4 := Body{ width: Fit }
                                }
                                due_5 := View{ width: Fill, height: Fit, flow: Right, margin: Inset{top: 6}
                                    due_name_5 := Body{ width: Fill }
                                    due_amount_5 := Body{ width: Fit }
                                }
                            }
                        }
                    }

                    // ================= TRANSACTIONS =================
                    ledger := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        ledger_grid := Ledger{}
                    }

                    // ================= BUDGET =================
                    budget := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        budget_bar := Panel{
                            width: Fill
                            height: 40
                            flow: Right
                            align: Align{x: 0.0, y: 0.5}
                            padding: Inset{left: 16, right: 16}
                            spacing: 10
                            budget_prev := Chip{ text: "‹" }
                            budget_month := Body{ width: 120 }
                            budget_next := Chip{ text: "›" }
                            budget_summary := Dim{ width: Fill }
                        }
                        budget_grid := Ledger{
                            cols: 5
                            default_col_width: 150.0
                        }
                    }

                    // ================= REPORTS =================
                    reports := ScrollYView{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 16
                        spacing: 14
                        flow_card := Card{
                            height: 240
                            flow_title := Title{ text: "Income and spending" }
                            flow_sub := Dim{}
                            flow_chart := Trend{}
                        }
                        payee_card := Card{
                            payee_title := Title{ text: "Biggest payees" }
                            pay_0 := BarRow{}
                            pay_1 := BarRow{}
                            pay_2 := BarRow{}
                            pay_3 := BarRow{}
                            pay_4 := BarRow{}
                            pay_5 := BarRow{}
                            pay_6 := BarRow{}
                            pay_7 := BarRow{}
                        }
                        subs_card := Card{
                            subs_title := Title{ text: "Recurring" }
                            subs_body := Dim{}
                        }
                    }

                    // ================= IMPORT =================
                    import := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: 16
                        spacing: 12
                        import_head := Card{
                            import_title := Title{ text: "Import a bank statement" }
                            import_hint := Dim{
                                text: "Pick a CSV your bank exported. Columns, date order and decimal style are detected; anything already in the ledger is skipped."
                            }
                            import_actions := View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 8
                                margin: Inset{top: 6}
                                import_pick := Primary{ text: "Choose CSV…" }
                                import_apply := Primary{ text: "Import" }
                                import_cancel := Chip{ text: "Discard" }
                            }
                            import_status := Body{ margin: Inset{top: 4} }
                        }
                        import_grid := Ledger{
                            cols: 5
                            default_col_width: 160.0
                        }
                    }
                }
            }
        }

        // ---- Bottom tab bar: the phone layout's navigation. Always in
        // the tree (a conditionally-built widget loses its state), just
        // hidden when there is a sidebar instead.
        tabbar := Panel{
            visible: false
            width: Fill
            height: 56
            flow: Right
            align: Align{x: 0.5, y: 0.5}
            padding: Inset{left: 6, right: 6}
            spacing: 2
            tab_overview := NavItem{ height: Fill, text: "Overview" }
            tab_ledger := NavItem{ height: Fill, text: "Ledger" }
            tab_budget := NavItem{ height: Fill, text: "Budget" }
            tab_reports := NavItem{ height: Fill, text: "Reports" }
            tab_import := NavItem{ height: Fill, text: "Import" }
        }
    }
}

/// Which screen is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Ledger,
    Budget,
    Reports,
    Import,
}

impl Screen {
    const ALL: [Screen; 5] =
        [Screen::Overview, Screen::Ledger, Screen::Budget, Screen::Reports, Screen::Import];

    fn title(self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Ledger => "Transactions",
            Screen::Budget => "Budget",
            Screen::Reports => "Reports",
            Screen::Import => "Import",
        }
    }

    fn view_id(self) -> &'static [LiveId] {
        match self {
            Screen::Overview => ids!(overview),
            Screen::Ledger => ids!(ledger),
            Screen::Budget => ids!(budget),
            Screen::Reports => ids!(reports),
            Screen::Import => ids!(import),
        }
    }
}

/// How much room there is, and therefore which app this is.
///
/// The thresholds are where the content stops fitting, not where a
/// particular device is: below ~700 points a register cannot show a
/// category and a balance as well as a payee, and a 248-point sidebar
/// costs more than it gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Phone: bottom tabs, one column, three-column register.
    Compact,
    /// Small window or tablet: sidebar, but the register drops the columns
    /// that only help when there is room to spare.
    Regular,
    /// Desktop: everything.
    Wide,
}

impl Layout {
    fn for_width(width: f64) -> Layout {
        if width < 700.0 {
            Layout::Compact
        } else if width < 1100.0 {
            Layout::Regular
        } else {
            Layout::Wide
        }
    }

    fn has_sidebar(self) -> bool {
        self != Layout::Compact
    }
}

/// The columns of the register, in order. Which of them are shown depends
/// on the layout — the first three are the ones a phone can afford.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Column {
    Date,
    Payee,
    Amount,
    Category,
    Account,
    Cleared,
    Balance,
}

impl Column {
    fn label(self) -> &'static str {
        match self {
            Column::Date => "Date",
            Column::Payee => "Payee",
            Column::Amount => "Amount",
            Column::Category => "Category",
            Column::Account => "Account",
            Column::Cleared => "",
            Column::Balance => "Balance",
        }
    }

    fn width(self, layout: Layout) -> f64 {
        match (self, layout) {
            (Column::Date, Layout::Compact) => 86.0,
            (Column::Date, _) => 104.0,
            (Column::Payee, Layout::Compact) => 150.0,
            (Column::Payee, _) => 230.0,
            (Column::Amount, _) => 110.0,
            (Column::Category, _) => 170.0,
            (Column::Account, _) => 130.0,
            (Column::Cleared, _) => 34.0,
            (Column::Balance, _) => 120.0,
        }
    }
}

/// The columns of the register for a layout.
///
/// The running balance is only shown for a single account: a balance
/// column over a mixed list jumps between accounts row by row and means
/// nothing, which is why every product hides it until you pick one.
fn columns_for(layout: Layout, one_account: bool) -> Vec<Column> {
    match layout {
        // A phone shows what a bank app shows: when, who, how much.
        Layout::Compact => vec![Column::Date, Column::Payee, Column::Amount],
        Layout::Regular => vec![
            Column::Date,
            Column::Payee,
            Column::Category,
            Column::Cleared,
            Column::Amount,
        ],
        Layout::Wide => {
            let mut columns = vec![
                Column::Date,
                Column::Payee,
                Column::Category,
                Column::Account,
                Column::Cleared,
                Column::Amount,
            ];
            if one_account {
                columns.push(Column::Balance);
            }
            columns
        }
    }
}

/// How much history a screen is looking at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Range {
    Month,
    Quarter,
    Year,
    All,
}

impl Range {
    fn resolve(self, today: Day, earliest: Day) -> DateRange {
        match self {
            Range::Month => DateRange::month(date::month_key(today)),
            Range::Quarter => DateRange { start: today - 89, end: today },
            Range::Year => DateRange::last_months(today, 12),
            Range::All => DateRange { start: earliest, end: today },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Range::Month => "this month",
            Range::Quarter => "the last 90 days",
            Range::Year => "the last 12 months",
            Range::All => "all time",
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Finance {
    #[deref]
    view: View,

    #[rust]
    backend: Backend,
    #[rust]
    ledger: Ledger,
    #[rust]
    today: Day,
    #[rust(Screen::Overview)]
    screen: Screen,
    #[rust(Layout::Wide)]
    layout: Layout,
    #[rust(Range::Month)]
    range: Range,
    /// Which account the register is filtered to; `None` = all of them.
    #[rust]
    account_filter: Option<Id>,
    #[rust]
    search: String,
    /// The register's rows, rebuilt whenever a filter changes: transaction
    /// index plus the running balance after it.
    #[rust]
    rows: Vec<(usize, i64)>,
    #[rust]
    budget_month: MonthKey,
    #[rust]
    status: String,
    #[rust]
    started: bool,
    #[rust]
    chrome_synced: bool,
    /// The statement being imported, once one is chosen.
    #[rust]
    import: Option<ImportState>,
}

impl Finance {
    fn currency(&self) -> Currency {
        self.ledger.base_currency
    }

    /// Load either the generated demo household or the native database.
    fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;

        let started = self.backend.start();
        self.today = started.today;
        self.ledger = started.ledger;
        self.status = started.status;
        let has_import = self.backend.has_import();
        self.widget(cx, ids!(nav_import)).set_visible(cx, has_import);
        self.widget(cx, ids!(tab_import)).set_visible(cx, has_import);
        self.view(cx, ids!(import)).set_visible(cx, false);

        self.budget_month = date::month_key(self.today);
        self.rebuild_rows();
        self.show_only_current_screen(cx);
        self.chrome_synced = false;
        self.redraw(cx);
    }

    /// The register's contents, after the account filter and the search.
    ///
    /// Recomputed from scratch on every change rather than maintained
    /// incrementally: it is a single pass over a few hundred thousand rows,
    /// which is cheaper than the bugs a cache would buy.
    fn rebuild_rows(&mut self) {
        let needle = self.search.trim().to_lowercase();
        let mut running: std::collections::HashMap<Id, i64> = self
            .ledger
            .accounts
            .iter()
            .map(|a| (a.id, a.opening_balance))
            .collect();

        // The ledger is stored date-ordered, so the running balance can be
        // accumulated in the same pass that filters.
        let mut indexed: Vec<usize> = (0..self.ledger.transactions.len()).collect();
        indexed.sort_by_key(|i| {
            let txn = &self.ledger.transactions[*i];
            (txn.date, txn.id)
        });

        self.rows.clear();
        for index in indexed {
            let txn = &self.ledger.transactions[index];
            let balance = running.entry(txn.account).or_insert(0);
            *balance += txn.amount;
            let balance = *balance;
            if self.account_filter.is_some_and(|id| id != txn.account) {
                continue;
            }
            if !needle.is_empty() {
                let category = txn.category_label(&self.ledger.categories).to_lowercase();
                let amount = format_minor(txn.amount, self.ledger.base_currency);
                let matches = txn.payee.to_lowercase().contains(&needle)
                    || txn.memo.to_lowercase().contains(&needle)
                    || category.contains(&needle)
                    || amount.contains(&needle);
                if !matches {
                    continue;
                }
            }
            self.rows.push((index, balance));
        }
        // Newest first, the way every register opens.
        self.rows.reverse();
    }

    fn earliest(&self) -> Day {
        self.ledger
            .transactions
            .iter()
            .map(|t| t.date)
            .min()
            .unwrap_or(self.today)
    }

    fn range(&self) -> DateRange {
        self.range.resolve(self.today, self.earliest())
    }

    /// Show the screen, and make the chrome agree with it.
    fn set_screen(&mut self, cx: &mut Cx, screen: Screen) {
        self.screen = screen;
        self.show_only_current_screen(cx);
        self.chrome_synced = false;
        self.redraw(cx);
    }

    /// The five screens are siblings in one `flow: Overlay`, so exactly one
    /// may be visible at a time — otherwise they draw on top of each other.
    fn show_only_current_screen(&mut self, cx: &mut Cx) {
        for screen in Screen::ALL {
            self.view(cx, screen.view_id())
                .set_visible(
                    cx,
                    screen == self.screen
                        && (screen != Screen::Import || self.backend.has_import()),
                );
        }
    }

    /// Apply the layout for this width: which chrome exists, and how dense
    /// the register is.
    fn apply_layout(&mut self, cx: &mut Cx, layout: Layout) {
        if self.layout == layout && self.chrome_synced {
            return;
        }
        self.layout = layout;
        self.view(cx, ids!(sidebar)).set_visible(cx, layout.has_sidebar());
        self.view(cx, ids!(tabbar)).set_visible(cx, !layout.has_sidebar());
        // The search field earns its width on a desktop; on a phone the
        // title and the chips are what fit.
        let compact = layout == Layout::Compact;
        self.widget(cx, ids!(range_quarter)).set_visible(cx, !compact);
        self.widget(cx, ids!(range_year)).set_visible(cx, !compact);
        self.widget(cx, ids!(range_all)).set_visible(cx, !compact);
        // Stat cards stack rather than shrink to illegibility.
        self.view(cx, ids!(stat_saved)).set_visible(cx, !compact);
        self.view(cx, ids!(stat_net)).set_visible(cx, layout != Layout::Compact);
    }

    /// Push every value the chrome shows. Cheap enough to run whenever
    /// something changed, rather than tracking what.
    fn sync_chrome(&mut self, cx: &mut Cx) {
        let currency = self.currency();
        let today = self.today;
        let range = self.range();

        self.label(cx, ids!(screen_title)).set_text(cx, self.screen.title());
        for (screen, id) in Screen::ALL.iter().zip([
            ids!(nav_overview),
            ids!(nav_ledger),
            ids!(nav_budget),
            ids!(nav_reports),
            ids!(nav_import),
        ]) {
            let active = *screen == self.screen;
            let mut item = self.button(cx, id);
            let color = if active { theme::rgb(0xe6edf3) } else { theme::rgb(0x9aa7b4) };
            let bg = if active { theme::rgb(0x1f3a63) } else { Vec4f::default() };
            script_apply_eval!(cx, item, {
                draw_bg +: { color: #(bg) }
                draw_text +: { color: #(color) }
            });
        }

        let worth = self.ledger.net_worth_on(today);
        self.label(cx, ids!(net_worth_value))
            .set_text(cx, &format_money(worth, currency));

        // Overview numbers.
        let flow = report::flow(&self.ledger, range);
        let saved = if flow.income > 0 {
            format!("{:.0}%", flow.net() as f64 / flow.income as f64 * 100.0)
        } else {
            "—".to_string()
        };
        for (id, label, value, note) in [
            (ids!(stat_in), "Money in", format_money(flow.income, currency), self.range.label()),
            (ids!(stat_out), "Money out", format_money(flow.expense, currency), self.range.label()),
            (ids!(stat_net), "Net", format_money(flow.net(), currency), self.range.label()),
            (ids!(stat_saved), "Kept", saved.clone(), "of what came in"),
        ] {
            let card = self.view(cx, id);
            card.label(cx, ids!(stat_label)).set_text(cx, label);
            card.label(cx, ids!(stat_value)).set_text(cx, &value);
            card.label(cx, ids!(stat_note)).set_text(cx, note);
        }

        // Net worth chart: 24 months of ends-of-month.
        let series = report::net_worth_series(&self.ledger, 24, today);
        let major = currency.decimals as i32;
        let scale = 10f64.powi(major);
        let values: Vec<f64> = series.iter().map(|(_, v)| *v as f64 / scale).collect();
        let high = values.iter().cloned().fold(f64::MIN, f64::max);
        let low = values.iter().cloned().fold(f64::MAX, f64::min);
        let marks = vec![
            (high, format_compact((high * scale) as i64, currency)),
            (low, format_compact((low * scale) as i64, currency)),
        ];
        self.finance_chart(cx, ids!(worth_chart)).set_area(cx, &values, marks);
        if let (Some(first), Some(last)) = (series.first(), series.last()) {
            let change = last.1 - first.1;
            self.label(cx, ids!(worth_sub)).set_text(
                cx,
                &format!(
                    "{} over 24 months",
                    if change >= 0 {
                        format!("up {}", format_money(change, currency))
                    } else {
                        format!("down {}", format_money(-change, currency))
                    }
                ),
            );
        }

        // Where the money went.
        let spending = report::spending_by_group(&self.ledger, range);
        let biggest = spending.first().map(|(_, v)| *v).unwrap_or(1).max(1);
        for (slot, id) in [
            ids!(cat_0),
            ids!(cat_1),
            ids!(cat_2),
            ids!(cat_3),
            ids!(cat_4),
            ids!(cat_5),
            ids!(cat_6),
            ids!(cat_7),
        ]
        .into_iter()
        .enumerate()
        {
            let row = self.widget(cx, id);
            match spending.get(slot) {
                Some((category, amount)) => {
                    row.set_visible(cx, true);
                    let name = category
                        .map(|c| self.ledger.categories.name(c).to_string())
                        .unwrap_or_else(|| "Uncategorized".to_string());
                    row.label(cx, ids!(bar_label)).set_text(cx, &name);
                    row.label(cx, ids!(bar_value))
                        .set_text(cx, &format_money(*amount, currency));
                    let fraction = (*amount as f64 / biggest as f64).clamp(0.0, 1.0);
                    set_bar(cx, &row, fraction, None);
                }
                None => row.set_visible(cx, false),
            }
        }

        // Coming up.
        let upcoming = report::upcoming(&self.ledger, 30, today);
        for (slot, (row_id, name_id, amount_id)) in [
            (ids!(due_0), ids!(due_name_0), ids!(due_amount_0)),
            (ids!(due_1), ids!(due_name_1), ids!(due_amount_1)),
            (ids!(due_2), ids!(due_name_2), ids!(due_amount_2)),
            (ids!(due_3), ids!(due_name_3), ids!(due_amount_3)),
            (ids!(due_4), ids!(due_name_4), ids!(due_amount_4)),
            (ids!(due_5), ids!(due_name_5), ids!(due_amount_5)),
        ]
        .into_iter()
        .enumerate()
        {
            let row = self.view(cx, row_id);
            match upcoming.get(slot) {
                Some(item) => {
                    row.set_visible(cx, true);
                    self.label(cx, name_id).set_text(
                        cx,
                        &format!("{}  ·  {}", item.payee, date::format_short(item.next_due)),
                    );
                    let mut amount = self.label(cx, amount_id);
                    amount.set_text(cx, &format_money(item.amount, currency));
                    let color = if item.amount < 0 {
                        theme::rgb(theme::CRITICAL)
                    } else {
                        theme::rgb(theme::GOOD)
                    };
                    script_apply_eval!(cx, amount, {
                        draw_text +: { color: #(color) }
                    });
                }
                None => row.set_visible(cx, false),
            }
        }

        // Reports.
        let months = report::monthly_flow(&self.ledger, 18, today);
        let income: Vec<f64> = months.iter().map(|(_, f)| f.income as f64 / scale).collect();
        let spend: Vec<f64> = months.iter().map(|(_, f)| -(f.expense as f64) / scale).collect();
        let labels: Vec<String> = months
            .iter()
            .map(|(key, _)| date::format_month(*key).split(' ').next().unwrap_or("").to_string())
            .collect();
        self.finance_chart(cx, ids!(flow_chart)).set_bars(cx, &income, &spend, labels);
        let average: i64 = if months.is_empty() {
            0
        } else {
            months.iter().map(|(_, f)| f.expense).sum::<i64>() / months.len() as i64
        };
        self.label(cx, ids!(flow_sub)).set_text(
            cx,
            &format!("Monthly net over 24 months · average spend {}", format_money(average, currency)),
        );

        let payees = report::top_payees(&self.ledger, range, 8);
        let biggest = payees.first().map(|(_, v, _)| *v).unwrap_or(1).max(1);
        for (slot, id) in [
            ids!(pay_0),
            ids!(pay_1),
            ids!(pay_2),
            ids!(pay_3),
            ids!(pay_4),
            ids!(pay_5),
            ids!(pay_6),
            ids!(pay_7),
        ]
        .into_iter()
        .enumerate()
        {
            let row = self.widget(cx, id);
            match payees.get(slot) {
                Some((payee, amount, count)) => {
                    row.set_visible(cx, true);
                    row.label(cx, ids!(bar_label))
                        .set_text(cx, &format!("{payee}  ({count})"));
                    row.label(cx, ids!(bar_value))
                        .set_text(cx, &format_money(*amount, currency));
                    let fraction = (*amount as f64 / biggest as f64).clamp(0.0, 1.0);
                    set_bar(cx, &row, fraction, None);
                }
                None => row.set_visible(cx, false),
            }
        }

        let subs = report::detected_subscriptions(&self.ledger, today);
        let monthly: i64 = subs
            .iter()
            .filter(|(_, _, r, _)| *r == Recurrence::Monthly)
            .map(|(_, amount, _, _)| -amount)
            .sum();
        let list = subs
            .iter()
            .take(8)
            .map(|(payee, amount, recurrence, due)| {
                format!(
                    "{payee} — {} {} · next {}",
                    format_money(*amount, currency),
                    recurrence.label().to_lowercase(),
                    date::format_short(*due)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.label(cx, ids!(subs_body)).set_text(
            cx,
            &format!(
                "{} recurring charges found, {} a month\n\n{list}",
                subs.len(),
                format_money(monthly, currency)
            ),
        );

        // Budget header.
        self.label(cx, ids!(budget_month))
            .set_text(cx, &date::format_month(self.budget_month));
        let lines = report::budget_lines(&self.ledger, self.budget_month);
        let assigned: i64 = lines.iter().map(|(_, l)| l.assigned).sum();
        let spent: i64 = lines.iter().map(|(_, l)| l.spent).sum();
        let over = lines.iter().filter(|(_, l)| l.available < 0).count();
        self.label(cx, ids!(budget_summary)).set_text(
            cx,
            &format!(
                "{} assigned · {} spent · {} left{}",
                format_money(assigned, currency),
                format_money(spent, currency),
                format_money(assigned - spent, currency),
                if over > 0 { format!(" · {over} over") } else { String::new() }
            ),
        );

        // Import.
        let import_line = match &self.import {
            Some(state) => format!(
                "{} · {} new, {} already here, {} unreadable{}",
                state.path,
                state.plan.new_count(),
                state.plan.duplicate_count(),
                state.plan.unreadable_count(),
                if state.ask_date_order {
                    "  ·  DATE ORDER IS A GUESS — check the preview"
                } else {
                    ""
                }
            ),
            None => self.status.clone(),
        };
        self.label(cx, ids!(import_status)).set_text(cx, &import_line);
        self.widget(cx, ids!(import_apply))
            .set_visible(cx, self.import.is_some());
        self.widget(cx, ids!(import_cancel))
            .set_visible(cx, self.import.is_some());

        self.chrome_synced = true;
    }

    /// Write the plan. Everything or nothing.
    fn commit_import(&mut self, cx: &mut Cx) {
        let Some(state) = self.import.take() else { return };
        match self.backend.commit_import(state) {
            Ok((ledger, status)) => {
                self.status = status;
                self.ledger = ledger;
                self.rebuild_rows();
                self.set_screen(cx, Screen::Ledger);
            }
            Err(error) => {
                self.status = format!("import failed, nothing written: {error}");
                self.chrome_synced = false;
                self.redraw(cx);
            }
        }
    }

    /// One register cell.
    fn ledger_cell(&self, row: usize, column: Column) -> (String, CellStyle) {
        let currency = self.ledger.base_currency;
        let Some((index, balance)) = self.rows.get(row).copied() else {
            return (String::new(), plain());
        };
        let txn = &self.ledger.transactions[index];
        match column {
            Column::Date => (date::format_short(txn.date), dim()),
            Column::Payee => {
                let mut style = plain();
                if txn.flagged {
                    style.color = Some(theme::rgb(theme::WARNING));
                }
                (txn.payee.clone(), style)
            }
            Column::Category => {
                let label = txn.category_label(&self.ledger.categories);
                let mut style = dim();
                if label.is_empty() && !txn.is_transfer() {
                    return ("— uncategorized".to_string(), CellStyle {
                        color: Some(theme::rgb(theme::WARNING)),
                        ..dim()
                    });
                }
                if txn.is_split() {
                    style.color = Some(theme::rgb(0xa371f7));
                }
                (label, style)
            }
            Column::Account => (self.ledger.account_name(txn.account).to_string(), dim()),
            Column::Cleared => (
                txn.cleared.mark().to_string(),
                CellStyle { align: 0.5, ..dim() },
            ),
            Column::Amount => (
                format_minor(txn.amount, currency),
                CellStyle {
                    color: Some(if txn.amount < 0 {
                        theme::rgb(theme::CRITICAL)
                    } else {
                        theme::rgb(theme::GOOD)
                    }),
                    align: 1.0,
                    bold: true,
                    ..plain()
                },
            ),
            Column::Balance => (
                format_minor(balance, currency),
                CellStyle { align: 1.0, ..dim() },
            ),
        }
    }

    /// One budget cell.
    fn budget_cell(&self, lines: &[(Id, BudgetLine)], row: usize, col: usize) -> (String, CellStyle) {
        let currency = self.ledger.base_currency;
        let Some((category, line)) = lines.get(row) else {
            return (String::new(), plain());
        };
        match col {
            0 => (self.ledger.categories.path(*category), plain()),
            1 => (format_minor(line.assigned, currency), CellStyle { align: 1.0, ..dim() }),
            2 => (format_minor(line.spent, currency), CellStyle { align: 1.0, ..dim() }),
            3 => (
                if line.carried != 0 {
                    format_minor(line.carried, currency)
                } else {
                    String::new()
                },
                CellStyle { align: 1.0, ..dim() },
            ),
            _ => (
                format_minor(line.available, currency),
                CellStyle {
                    color: Some(match line.state() {
                        BudgetState::Overspent => theme::rgb(theme::CRITICAL),
                        BudgetState::Untouched => theme::rgb(0x6b7784),
                        _ => theme::rgb(theme::GOOD),
                    }),
                    align: 1.0,
                    bold: true,
                    ..plain()
                },
            ),
        }
    }

    /// One preview cell of an import.
    fn import_cell(&self, state: &ImportState, row: usize, col: usize) -> (String, CellStyle) {
        let currency = self.ledger.base_currency;
        let Some(candidate) = state.plan.rows.get(row) else {
            return (String::new(), plain());
        };
        use crate::import::RowStatus;
        let faded = matches!(candidate.status, RowStatus::Duplicate | RowStatus::Unreadable);
        let base = if faded { dim() } else { plain() };
        match col {
            0 => (
                match candidate.status {
                    RowStatus::New => "new".to_string(),
                    RowStatus::Duplicate => "already here".to_string(),
                    RowStatus::Unreadable => "unreadable".to_string(),
                },
                CellStyle {
                    color: Some(match candidate.status {
                        RowStatus::New => theme::rgb(theme::GOOD),
                        RowStatus::Duplicate => theme::rgb(0x6b7784),
                        RowStatus::Unreadable => theme::rgb(theme::CRITICAL),
                    }),
                    ..base
                },
            ),
            1 => (
                if candidate.status == RowStatus::Unreadable {
                    String::new()
                } else {
                    date::format_short(candidate.txn.date)
                },
                base,
            ),
            2 => (candidate.txn.payee.clone(), base),
            3 => (
                self.ledger.categories.path(candidate.txn.category.unwrap_or(0)),
                base,
            ),
            _ => (
                format_minor(candidate.txn.amount, currency),
                CellStyle { align: 1.0, bold: !faded, ..base },
            ),
        }
    }
}

/// Set a bar's length. The meter shader takes the fraction directly, so
/// there is nothing to measure and nothing to re-measure on a resize.
fn set_bar(cx: &mut Cx, row: &WidgetRef, fraction: f64, color: Option<Vec4f>) {
    row.meter(cx, ids!(bar_meter)).set(cx, fraction, color, -1.0);
}

fn plain() -> CellStyle {
    CellStyle { bg: None, color: None, align: 0.0, bold: false, font_scale: 1.0 }
}

fn dim() -> CellStyle {
    CellStyle { color: Some(theme::rgb(0x9aa7b4)), ..plain() }
}

impl Widget for Finance {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.start(cx);

        // The layout follows the window, one frame behind on a resize —
        // which is invisible, because a resize redraws continuously.
        let width = self.view.area().rect(cx).size.x;
        if width > 1.0 {
            let layout = Layout::for_width(width);
            if layout != self.layout {
                self.apply_layout(cx, layout);
            }
        }
        if !self.chrome_synced {
            self.sync_chrome(cx);
        }

        let columns = columns_for(self.layout, self.account_filter.is_some());
        // The grid's own width, for handing the slack to one column.
        let grid_width = self.widget(cx, ids!(ledger_grid)).area().rect(cx).size.x;
        let budget_lines = report::budget_lines(&self.ledger, self.budget_month);

        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            // The account list.
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                let accounts: Vec<&Account> =
                    self.ledger.accounts.iter().filter(|a| !a.closed).collect();
                list.set_item_range(cx, 0, accounts.len());
                while let Some(index) = list.next_visible_item(cx) {
                    // PortalList hands out ids past the end to fill the
                    // viewport; drawing those would repeat the last row.
                    if index >= accounts.len() {
                        continue;
                    }
                    let account = accounts[index];
                    let item = list.item(cx, index, live_id!(Account));
                    let balance = self.ledger.balance(account.id);
                    item.label(cx, ids!(acc_name)).set_text(cx, &account.name);
                    item.label(cx, ids!(acc_kind)).set_text(
                        cx,
                        &format!("{} · {}", account.kind.label(), account.institution),
                    );
                    let mut money = item.label(cx, ids!(acc_balance));
                    money.set_text(cx, &format_compact(balance, account.currency));
                    let color = if balance < 0 {
                        theme::rgb(theme::CRITICAL)
                    } else {
                        theme::rgb(0xe6edf3)
                    };
                    script_apply_eval!(cx, money, {
                        draw_text +: { color: #(color) }
                    });
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }

            // The three grids.
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            match self.screen {
                Screen::Ledger => {
                    grid.set_grid_size(self.rows.len(), columns.len());
                    grid.set_col_labels(columns.iter().map(|c| c.label().to_string()).collect());
                    let fixed: f64 = columns
                        .iter()
                        .filter(|c| **c != Column::Payee)
                        .map(|c| c.width(self.layout))
                        .sum();
                    // Payee absorbs the remainder, so the register fills
                    // the window at any width instead of leaving a gutter.
                    let payee = (grid_width - fixed - 2.0).max(120.0);
                    for (index, column) in columns.iter().enumerate() {
                        grid.set_col_width(
                            index,
                            if *column == Column::Payee { payee } else { column.width(self.layout) },
                        );
                    }
                    grid.set_default_sizes(
                        cx,
                        140.0,
                        if self.layout == Layout::Compact { 40.0 } else { 26.0 },
                    );
                    while let Some(cell) = grid.next_cell(cx) {
                        let Some(column) = columns.get(cell.col) else { continue };
                        let (text, style) = self.ledger_cell(cell.row, *column);
                        grid.cell_text_styled(cx, &cell, &text, style);
                    }
                }
                Screen::Budget => {
                    grid.set_grid_size(budget_lines.len(), 5);
                    grid.set_col_labels(
                        ["Category", "Assigned", "Spent", "Carried", "Available"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    );
                    grid.set_col_width(0, 220.0);
                    while let Some(cell) = grid.next_cell(cx) {
                        let (text, style) = self.budget_cell(&budget_lines, cell.row, cell.col);
                        grid.cell_text_styled(cx, &cell, &text, style);
                    }
                }
                Screen::Import => {
                    let rows = self.import.as_ref().map(|s| s.plan.rows.len()).unwrap_or(0);
                    grid.set_grid_size(rows, 5);
                    grid.set_col_labels(
                        ["", "Date", "Payee", "Category", "Amount"]
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    );
                    grid.set_col_width(0, 110.0);
                    grid.set_col_width(2, 240.0);
                    while let Some(cell) = grid.next_cell(cx) {
                        let (text, style) = match &self.import {
                            Some(state) => self.import_cell(state, cell.row, cell.col),
                            None => (String::new(), plain()),
                        };
                        grid.cell_text_styled(cx, &cell, &text, style);
                    }
                }
                _ => {}
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        self.widget_match_event(cx, event, scope);
    }
}

impl WidgetMatchEvent for Finance {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        // Navigation: the sidebar and the tab bar drive the same screens.
        for (screen, nav, tab) in [
            (Screen::Overview, ids!(nav_overview), ids!(tab_overview)),
            (Screen::Ledger, ids!(nav_ledger), ids!(tab_ledger)),
            (Screen::Budget, ids!(nav_budget), ids!(tab_budget)),
            (Screen::Reports, ids!(nav_reports), ids!(tab_reports)),
        ] {
            if self.button(cx, nav).clicked(actions) || self.button(cx, tab).clicked(actions) {
                self.set_screen(cx, screen);
            }
        }
        if self.backend.has_import()
            && (self.button(cx, ids!(nav_import)).clicked(actions)
                || self.button(cx, ids!(tab_import)).clicked(actions))
        {
            self.set_screen(cx, Screen::Import);
        }

        for (range, id) in [
            (Range::Month, ids!(range_month)),
            (Range::Quarter, ids!(range_quarter)),
            (Range::Year, ids!(range_year)),
            (Range::All, ids!(range_all)),
        ] {
            if self.button(cx, id).clicked(actions) {
                self.range = range;
                self.chrome_synced = false;
                self.redraw(cx);
            }
        }

        if self.button(cx, ids!(budget_prev)).clicked(actions) {
            self.budget_month -= 1;
            self.chrome_synced = false;
            self.redraw(cx);
        }
        if self.button(cx, ids!(budget_next)).clicked(actions) {
            self.budget_month += 1;
            self.chrome_synced = false;
            self.redraw(cx);
        }

        if self.backend.has_import() && self.button(cx, ids!(import_pick)).clicked(actions) {
            self.backend.pick_statement(cx);
        }
        if self.backend.has_import() && self.button(cx, ids!(import_apply)).clicked(actions) {
            self.commit_import(cx);
        }
        if self.button(cx, ids!(import_cancel)).clicked(actions) {
            self.import = None;
            self.chrome_synced = false;
            self.redraw(cx);
        }

        // Search filters the register as it is typed: the whole ledger is
        // in memory, so there is no reason to make anyone press Enter.
        if let Some(text) = self.text_input(cx, ids!(search_input)).changed(actions) {
            self.search = text;
            self.rebuild_rows();
            self.set_screen(cx, Screen::Ledger);
        }

        // The account list filters the register: clicking the account
        // already shown clears the filter, so the row is a toggle.
        let accounts: Vec<Id> = self
            .ledger
            .accounts
            .iter()
            .filter(|a| !a.closed)
            .map(|a| a.id)
            .collect();
        for (index, item) in self.portal_list(cx, ids!(accounts_list)).items_with_actions(actions) {
            if item.button(cx, ids!(acc_hit)).clicked(actions) {
                let picked = accounts.get(index as usize).copied();
                self.account_filter = if self.account_filter == picked { None } else { picked };
                self.rebuild_rows();
                self.set_screen(cx, Screen::Ledger);
            }
        }

        if let Some(prepared) =
            self.backend.prepare_from_actions(actions, &self.ledger, self.account_filter)
        {
            match prepared {
                Ok(state) => {
                    self.import = Some(state);
                    self.set_screen(cx, Screen::Import);
                }
                Err(error) => {
                    self.status = error;
                    self.chrome_synced = false;
                    self.redraw(cx);
                }
            }
        }
    }
}
