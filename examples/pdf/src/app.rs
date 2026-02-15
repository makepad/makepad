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

    fn try_load_pdf(&mut self, cx: &mut Cx) {
        let Some(data) = self.pdf_data.take() else {
            return;
        };

        let direct = self.ui.widget(cx, ids!(pdf_view));
        let path_main = self.ui.widget(cx, ids!(main_window.pdf_view));
        let path_body = self.ui.widget(cx, ids!(main_window.body.pdf_view));
        let flood = self.ui.widget_flood(cx, ids!(pdf_view));

        let pdf_view = if path_body.borrow::<PdfView>().is_some() {
            path_body
        } else if direct.borrow::<PdfView>().is_some() {
            direct
        } else if flood.borrow::<PdfView>().is_some() {
            flood
        } else if path_main.borrow::<PdfView>().is_some() {
            path_main
        } else {
            direct
        };

        let Some(mut inner) = pdf_view.borrow_mut::<PdfView>() else {
            self.pdf_data = Some(data);
            self.ui.redraw(cx);
            return;
        };
        inner.load_pdf_data(cx, data);
        self.ui.redraw(cx);
        cx.redraw_all();
    }
}

#[derive(Script)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    pdf_data: Option<Vec<u8>>,
}

impl ScriptHook for App {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        vm.set_ui(&self.ui);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();

        self.pdf_data = Some(if let Some(path) = find_pdf_path_arg(&args) {
            match std::fs::read(&path) {
                Ok(data) => data,
                Err(_) => generate_demo_pdf(),
            }
        } else {
            generate_demo_pdf()
        });
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if matches!(event, Event::Draw(_)) {
            self.try_load_pdf(cx);
        }
    }
}

fn find_pdf_path_arg(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--pdf" || arg == "--file" {
            if let Some(path) = iter.next() {
                return Some(path.clone());
            }
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        let looks_like_pdf = arg.to_ascii_lowercase().ends_with(".pdf");
        let exists_as_path = std::path::Path::new(arg).exists();
        if !looks_like_pdf && !exists_as_path {
            continue;
        }
        return Some(arg.clone());
    }
    None
}

fn generate_demo_pdf() -> Vec<u8> {
    makepad_pdf_parse::generate_test_pdf(25)
}
