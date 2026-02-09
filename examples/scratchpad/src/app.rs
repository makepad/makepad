use makepad_widgets2::*;

app_main!(App);

// ===================================================================
// EXAMPLE: Web Search via DuckDuckGo
// Real HTTP + HTML parsing — no API keys, no cookies, no JS.
// Uses DuckDuckGo's HTML-only endpoint which returns static HTML
// with well-structured CSS classes, perfect for parse_html().
// ===================================================================

script_mod! {
    use mod.prelude.widgets.*

    // ---- Templates ----

    let ResultCard = RoundedView{
        width: Fill height: Fit
        padding: 14 flow: Down spacing: 6
        draw_bg.color: #ffffff
        draw_bg.border_radius: 8.0
        draw_bg.border_size: 1.0
        draw_bg.border_color: #xe8e8e8

        // Title (looks like a link)
        title := Label{text: "" draw_text.color: #x1a0dab draw_text.text_style: theme.font_bold{font_size: 13}}

        // Display URL
        url := Label{text: "" draw_text.color: #x006621 draw_text.text_style.font_size: 9}

        // Snippet
        snippet := Label{text: "" draw_text.color: #x545454 draw_text.text_style.font_size: 11}
    }

    let LoadingCard = RoundedView{
        width: Fill height: 80
        padding: 14
        draw_bg.color: #fafafa
        draw_bg.border_radius: 8.0
        align: Center
        LoadingSpinner{width: 24 height: 24}
    }

    let EmptyState = View{
        width: Fill height: 200
        align: Center
        flow: Down spacing: 8
        Label{text: "No results yet" draw_text.color: #999999 draw_text.text_style.font_size: 13}
        Label{text: "Type a query and hit Search" draw_text.color: #cccccc draw_text.text_style.font_size: 10}
    }

    // ---- Data ----

    var query = ""
    var results = []
    var loading = false
    var error = ""

    // ---- UI ----

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: vec4(0.96, 0.96, 0.97, 1.0)
                window.inner_size: vec2(650, 700)
                body +: {
                    flow: Down spacing: 0
                    align: Align{x: 0.5}

                    // Search bar
                    RoundedView{
                        width: Fill height: Fit
                        padding: 16 flow: Down spacing: 10
                        draw_bg.color: #ffffff
                        draw_bg.border_radius: 0.0

                        Label{text: "Web Search" draw_text.color: #222222 draw_text.text_style: theme.font_bold{font_size: 18}}

                        View{
                            width: Fill height: Fit flow: Right spacing: 8
                            align: Align{y: 0.5}
                            TextInput{
                                width: Fill height: Fit
                                empty_text: "Search the web..."
                                on_change: |text| { query = text }
                                on_return: |text| { query = text; do_search() }
                            }
                            Button{
                                text: "Search"
                                on_click: || do_search()
                            }
                        }

                        // AI row
                        View{
                            width: Fill height: Fit flow: Right spacing: 8
                            align: Align{y: 0.5}
                            Button{
                                text: "AI: Summarize results"
                                on_click: || ai_summarize()
                            }
                            Filler{}
                            render: || {
                                if loading {
                                    Label{text: "Searching..." draw_text.color: #x7744cc draw_text.text_style.font_size: 9}
                                }
                            }
                        }
                    }

                    Hr{}

                    // Results area
                    ScrollYView{
                        width: Fill height: Fill
                        padding: 16 flow: Down spacing: 10

                        // Status bar
                        View{
                            width: Fill height: Fit
                            render: || {
                                if error != "" {
                                    Label{text: error draw_text.color: #xcc4444 draw_text.text_style.font_size: 10}
                                }
                                else if !loading && results.len() > 0 {
                                    Label{text: results.len() + " results" draw_text.color: #888888 draw_text.text_style.font_size: 10}
                                }
                            }
                        }

                        // Result cards
                        View{
                            width: Fill height: Fit
                            flow: Down spacing: 10
                            render: || {
                                if loading {
                                    LoadingCard{}
                                    LoadingCard{}
                                    LoadingCard{}
                                }
                                else if results.len() == 0 {
                                    EmptyState{}
                                }
                                else {
                                    for result in results {
                                        ResultCard{
                                            title.text: result.title
                                            url.text: result.url
                                            snippet.text: result.snippet
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- Logic (ground floor) ----

    fn do_search(){
        if query == "" return
        loading = true
        error = ""
        results = []
        render()

        // DuckDuckGo's HTML endpoint: no cookies, no JS, no API key.
        // Returns static HTML with CSS classes we can parse.
        let req = net.HttpRequest{
            url: "https://html.duckduckgo.com/html/?q=" + query
            method: net.HttpMethod.GET
            headers: {"User-Agent": "MakepadApp/1.0"}
        }
        net.http_request(req) do net.HttpEvents{
            on_response: |res| {
                loading = false

                // Parse the HTML response
                let doc = res.body.to_string().parse_html()

                // DuckDuckGo HTML uses these classes:
                //   a.result__a        — title + href
                //   a.result__snippet  — description text
                let links = doc.query("a.result__a").array()
                let snippets = doc.query("a.result__snippet").array()

                results = []
                for link, i in links {
                    results.push({
                        title: link.text
                        url: link.attr("href")
                        snippet: if i < snippets.len() snippets[i].text else ""
                    })
                }
                render()
            }
            on_error: |e| {
                loading = false
                error = "Search failed: " + e.message
                render()
            }
        }
    }

    fn ai_summarize(){
        if results.len() == 0 return
        ai.send({
            request: "summarize"
            query: query
            results: results
        })
    }

    // ============================================================
    // BELOW THIS LINE: the AI appends snippets in response.
    // Each snippet runs in the same scope — it can read/write
    // all vars, call all fns, and render() the UI.
    // ============================================================

    // --- AI response to: summarize ---
    // (the AI read the search results and composed a summary)
    // ai would append something like:
    //   results = results.slice(0, 3)   // trim to top 3
    //   render()
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
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
