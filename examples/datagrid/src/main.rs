pub use makepad_widgets;

use makepad_widgets::*;

mod sheet_engine;
mod tab_bigdata;
mod tab_charts;
mod tab_pixels;
mod tab_sheets;
mod tab_widgets;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let GridTab = RectView{
        height: Fill width: Fill
        flow: Down
        padding: 0
        spacing: 0.
    }

    let AppDock = Dock{
        height: Fill width: Fill

        root := DockTabs{
            tabs: [@tSheets @tBigData @tPixels @tWidgets @tCharts]
            selected: 0
            closable: false
        }

        tSheets := DockTab{name: "Sheets" template: @PermanentTab kind: @TabSheets}
        tBigData := DockTab{name: "Big Data (1B cells)" template: @PermanentTab kind: @TabBigData}
        tPixels := DockTab{name: "Pixels" template: @PermanentTab kind: @TabPixels}
        tWidgets := DockTab{name: "Widget Cells" template: @PermanentTab kind: @TabWidgets}
        tCharts := DockTab{name: "Charts" template: @PermanentTab kind: @TabCharts}

        TabSheets := GridTab{SheetsTab{}}
        TabBigData := GridTab{BigDataTab{}}
        TabPixels := GridTab{PixelsTab{}}
        TabWidgets := GridTab{WidgetCellsTab{}}
        TabCharts := GridTab{ChartsTab{}}
    }

    mod.gc.set_static(AppDock)
    mod.gc.run()

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1480 920)
                body +: {
                    flow: Down
                    spacing: 0.
                    margin: 0.

                    dock := AppDock{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::tab_sheets::script_mod(vm);
        crate::tab_bigdata::script_mod(vm);
        crate::tab_pixels::script_mod(vm);
        crate::tab_widgets::script_mod(vm);
        crate::tab_charts::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
