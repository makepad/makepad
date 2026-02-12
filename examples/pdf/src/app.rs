use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 700)
                window.title: "PDF Viewer"
                body +: {
                    pdf_view := PdfView {
                        width: Fill
                        height: Fill
                    }
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    loaded: bool,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        if !self.loaded {
            self.loaded = true;
            // Check command line args for a PDF path
            let args: Vec<String> = std::env::args().collect();
            let pdf_data = if args.len() > 1 {
                // Load from file
                match std::fs::read(&args[1]) {
                    Ok(data) => {
                        log!("Loading PDF: {} ({} bytes)", args[1], data.len());
                        data
                    }
                    Err(e) => {
                        log!("Failed to read {}: {}", args[1], e);
                        generate_demo_pdf()
                    }
                }
            } else {
                log!("No PDF file specified, using generated demo. Usage: cargo run -p makepad-example-pdf -- <file.pdf>");
                generate_demo_pdf()
            };

            self.ui.pdf_view(cx, ids!(pdf_view)).load_pdf(cx, pdf_data);
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        cx.with_widget_tree(|cx| {
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
        });
    }
}

fn generate_demo_pdf() -> Vec<u8> {
    makepad_pdf_parse::generate_test_pdf(25)
}
