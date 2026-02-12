use makepad_widgets2::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.net

    // ---- Templates ----

    let ResultCard = RoundedView{
        width: Fill height: Fit
        padding: 14 flow: Down spacing: 6
        draw_bg.color: #x2a2a2a
        draw_bg.border_radius: 8.0
        draw_bg.border_size: 1.0
        draw_bg.border_color: #x3a3a3a

        title := Label{text: "" draw_text.color: #x6af draw_text.text_style: theme.font_bold{font_size: 13}}
        url := Label{text: "" draw_text.color: #x5a5 draw_text.text_style.font_size: 9}
        snippet := Label{text: "" draw_text.color: #xaaa draw_text.text_style.font_size: 11}
    }

    let LoadingCard = RoundedView{
        width: Fill height: 80
        padding: 14
        draw_bg.color: #x222
        draw_bg.border_radius: 8.0
        align: Center
        LoadingSpinner{width: 24 height: 24}
    }

    let EmptyState = View{
        width: Fill height: 200
        align: Center
        flow: Down spacing: 8
        Label{text: "No results yet" draw_text.color: #x666 draw_text.text_style.font_size: 13}
        Label{text: "Type a query and hit Search" draw_text.color: #x444 draw_text.text_style.font_size: 10}
    }

    // ---- UI ----
    let results = []

    let app = load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(0.12, 0.12, 0.14, 1.0)
                window.inner_size: vec2(650, 700)
                body +: {
                    flow: Down spacing: 0
                    align: Align{x: 0.5}

                    // Search bar
                    RoundedView{
                        width: Fill height: Fit
                        padding: 16 flow: Down spacing: 10
                        draw_bg.color: #x1a1a1c
                        draw_bg.border_radius: 0.0

                        Label{text: "Web Search" draw_text.color: #xddd draw_text.text_style: theme.font_bold{font_size: 18}}

                        View{
                            width: Fill height: Fit flow: Right spacing: 8
                            align: Align{y: 0.5}
                            search_input := TextInput{
                                width: Fill height: Fit
                                empty_text: "Search the web..."
                                on_return: || ui.search_button.on_click()
                            }
                            search_button := Button{
                                text: "Search"
                                on_click: ||{
                                    do_search(ui.search_input.text())
                                }
                            }
                        }

                        View{
                            width: Fill height: Fit flow: Right spacing: 8
                            align: Align{y: 0.5}
                            ai_button := Button{
                                text: "AI: Summarize results"
                            }
                        }
                    }

                    Hr{}

                    // Results area
                    results := ScrollYView{
                        width: Fill height: Fill
                        padding: 16 flow: Down spacing: 10
                        render: ||{
                            if results.len() == 0
                                EmptyState{}
                            else for result in results{
                                ResultCard{
                                    title.text: result.title
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Logic ----
    // These use script features (var, fn) that run in the script VM.
    // They will work once the script engine supports them fully.

    fn do_search(query){
        let req = net.HttpRequest{
            url: "https://html.duckduckgo.com/html/?q=" + query
            method: net.HttpMethod.GET
            headers: {"User-Agent": "MakepadApp/1.0"}
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| {
                let doc = res.body.to_string().parse_html()
                let links = doc.query("a.result__a").array()
                let snippets = doc.query("a.result__snippet").array()
                for i, link in links {
                    results.push({
                        title: link.text
                        url: link.attr("href")
                        snippet: if i < snippets.len() snippets[i].text else ""
                    })
                }
                ui.results.render()
            }
        }
    }

    app
}

// ---- Rust boilerplate ----

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
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
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        cx.with_widget_tree(|cx| {
            self.match_event(cx, event);
            self.ui.handle_event(cx, event, &mut Scope::empty());
        });
    }
}
