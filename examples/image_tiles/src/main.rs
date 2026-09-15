//! A pannable, zoomable wall of pictures over a baked tile library.
//!
//! Bake a library first (the manifest beside this example is a start):
//! ```text
//! cargo run -p makepad-image-tiles --release --bin image-tiles-bake -- \
//!     examples/image_tiles/manifest.tsv
//! ```
//! then run this viewer from the same directory. `IMAGE_TILES_HOME` names a
//! different library; the default is the nearest `local/image-tiles`.
//! Wheel zooms around the cursor, drag pans, click logs the picture.

pub use makepad_widgets;
use makepad_image_tiles::TileGridAction;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 800)
                body +: {
                    grid_wrap := View{
                        width: Fill
                        height: Fill
                        grid := TileGrid{}
                    }
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
    fn handle_startup(&mut self, cx: &mut Cx) {
        // The grid owns its draw list: without this, any sibling redraw
        // re-runs the grid's draw_walk and re-uploads the whole instance
        // buffer every frame.
        if let Some(mut wrap) = self.ui.view(cx, ids!(grid_wrap)).borrow_mut() {
            wrap.set_optimize(cx, ViewOptimize::DrawList);
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(widget_action) = action.as_widget_action() else {
                continue;
            };
            match widget_action.cast() {
                TileGridAction::Clicked { item, title, link, url } => {
                    log!("picture {item}: {title} — {}", if link.is_empty() { url } else { link });
                }
                TileGridAction::Opened { count, error: None } => {
                    log!("library open: {count} picture(s)");
                }
                TileGridAction::Opened { error: Some(e), .. } => {
                    log!("no library: {e} — bake one with image-tiles-bake first");
                }
                _ => {}
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_image_tiles::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
