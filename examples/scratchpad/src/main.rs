pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.net

    let results = []

    let ImageCard = RoundedView{
        width: Fill height: Fit padding: 10 flow: Down spacing: 6
        draw_bg+: {color: #x2a2a2a border_radius: 8.0 border_size: 1.0 border_color: #x3a3a3a}
        thumb := Image{width: Fill height: 120 fit: ImageFit.Smallest}
        title := Label{text: "" width: Fill draw_text.color: #xddd draw_text.text_style.font_size: 11}
        source := Label{text: "" draw_text.color: #x5a5 draw_text.text_style.font_size: 9}
    }

    fn fetch(url, extra_headers){
        let p = promise()
        let h = {"User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"}
        if extra_headers != nil { for k, v in extra_headers { h[k] = v } }
        net.http_request(net.HttpRequest{url: url method: net.HttpMethod.GET headers: h}) do net.HttpEvents{
            on_response: |res| p.resolve(res)
            on_error: |err| p.resolve(nil)
        }
        p
    }

    fn do_search(query){
        let q = query.url_encode()
        let page = fetch("https://duckduckgo.com/?q=" + q + "&iax=images&ia=images", nil).await()
        if page == nil {return}

        let vqd = ""
        let parts = page.body.to_string().split("vqd=\"")
        if parts.len() > 1 { vqd = parts[1].split("\"")[0] }
        if vqd == "" {return}

        let res = fetch("https://duckduckgo.com/i.js?l=us-en&o=json&q=" + q + "&vqd=" + vqd + "&f=,,,,,&p=1", {"Referer": "https://duckduckgo.com/"}).await()
        if res == nil {return}

        let data = res.body.to_string().parse_json()
        if data == nil || data.results == nil {return}

        results.clear()
        for img in data.results {
            results.push({title: img.title source: img.source thumbnail: img.thumbnail image: img.image})
        }
        ui.results_view.render()
    }

    let app = startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(0.12, 0.12, 0.14, 1.0)
                window.inner_size: vec2(700, 750)
                body +: {
                    flow: Down spacing: 0 align: Align{x: 0.5}

                    RoundedView{
                        width: Fill height: Fit padding: 16 flow: Down spacing: 10
                        draw_bg+: {color: #x1a1a1c border_radius: 0.0}
                        Label{text: "Image Search" draw_text.color: #xddd draw_text.text_style: theme.font_bold{font_size: 18}}
                        View{
                            width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}
                            search_input := TextInput{
                                width: Fill height: Fit
                                empty_text: "Search for images..."
                                on_return: || ui.search_button.on_click()
                            }
                            search_button := Button{
                                text: "Search"
                                on_click: || do_search(ui.search_input.text())
                            }
                        }
                    }
                    Hr{}
                    results_view := ScrollYView{
                        width: Fill height: Fill padding: 16 flow: Down spacing: 10
                        new_batch: true
                        on_render: ||{
                            if results.len() == 0 {
                                View{
                                    width: Fill height: 200 align: Center flow: Down spacing: 8
                                    Label{text: "Image Search" draw_text.color: #x666 draw_text.text_style.font_size: 13}
                                    Label{text: "Type a query and hit Search" draw_text.color: #x444 draw_text.text_style.font_size: 10}
                                }
                            }
                            else for result in results {
                                ImageCard{
                                    thumb.src: http_resource(result.thumbnail)
                                    title.text: result.title
                                    source.text: result.source
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    app
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
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
