pub use makepad_widgets;

use makepad_widgets::*;

// `ScriptArrayStorage` (to read the U8 byte array argument) and the error macro
// are not part of the `makepad_widgets::*` glob — pull them from the script crate.
use crate::makepad_widgets::makepad_script::{script_err_type_mismatch, ScriptArrayStorage};

app_main!(App);

// Fetch a *compressed* WOFF2 font over the network, decompress it in the app, and
// hand the finished sfnt bytes to the text engine — the makepad equivalent of
// Figma's "download font bytes → decode → draw".
//
// The draw crate never touches WOFF2: it only ever parses sfnt (TTF/OTF). All the
// WOFF2 handling lives here in the example:
//   1. `net.http_request(...)` fetches the raw .woff2 bytes (`res.body`).
//   2. `woff2.decompress(bytes)` — a native fn registered below, wrapping the
//      standalone `makepad-woff2` crate — turns them into sfnt bytes.
//   3. `binary_resource(sfnt)` wraps those bytes in an in-memory resource handle,
//      which is fed to the label's `FontMember` exactly like any other font.
//
// The URL below serves a WOFF2 with permissive CORS. To render your own font,
// point the request at any reachable .woff2 URL (a local static server is simplest
// — same origin needs no CORS).
script_mod! {
    use mod.prelude.widgets.*
    use mod.net
    use mod.woff2

    // Populated once the font has been fetched and decompressed; nil until then.
    let font_res = nil

    fn load_font(){
        let url = "https://fonts.gstatic.com/s/googlesansflex/v5/t5s6IQcYNIWbFgDgAAzZ34auoVyXkJCOvp3SFWJbN5hF8Ju1x5tKByN2l9sI40swNJwakXdYAZzz0jbnJ4qFQO5tGjLvDSkV4DyKMo6qQzwliVdHySgxyRg2eWQTmw.woff2"
        net.http_request(net.HttpRequest{url: url method: net.HttpMethod.GET}) do net.HttpEvents{
            on_response: |res| {
                // `res.body` is the raw WOFF2 bytes. Decompress to sfnt, then wrap
                // the finished bytes in an in-memory resource for the text engine.
                let sfnt = woff2.decompress(res.body)
                if sfnt != nil {
                    font_res = binary_resource(sfnt)
                    ui.main_view.render()
                }
            }
        }
    }

    let app = startup() do #(App::script_component(vm)){
        ui: Root{
            on_startup:||{
                load_font()
            }
            main_window := Window{
                window.inner_size: vec2(560, 240)
                pass.clear_color: vec4(0.12, 0.12, 0.14, 1.0)
                body +: {
                    flow: Down
                    spacing: 12
                    align: Center
                    main_view := View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        align: Center
                        on_render: ||{
                            // NOTE: give each branch's widget an explicit name
                            // (`fetching :=` / `loaded :=`). Without a name, `on_render`
                            // children get positional anonymous ids, so the `if` and
                            // `else` labels would collide on the same id and the branch
                            // flip would update-in-place instead of swapping widgets —
                            // leaving the old label on screen.
                            if font_res == nil {
                                fetching := Label{
                                    text: "Fetching web font…"
                                    draw_text.color: #fff
                                    draw_text.text_style.font_size: 20.0
                                }
                            }
                            else {
                                loaded := Label{
                                    text: "Web font: Hello 你好 \n fetched & decompressed over HTTP"
                                    draw_text.color: #fff
                                    draw_text +: {
                                        text_style: TextStyle{
                                            font_family: FontFamily{
                                                latin := FontMember{
                                                    res: font_res
                                                    asc: -0.1 desc: 0.0
                                                }
                                            }
                                            font_size: 28.0
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
    app
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        register_woff2(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

// Registers a `woff2` module exposing `woff2.decompress(bytes) -> bytes`, bridging
// the standalone `makepad-woff2` decompressor into script. Returns nil if the input
// isn't a byte array or isn't a valid WOFF2 stream.
fn register_woff2(vm: &mut ScriptVm) {
    let woff2 = vm.new_module(id_lut!(woff2));
    vm.add_method(
        woff2,
        id_lut!(decompress),
        script_args_def!(data = NIL),
        move |vm, args| {
            let data = script_value!(vm, args.data);
            let Some(array) = data.as_array() else {
                return script_err_type_mismatch!(vm.trap(), "woff2.decompress expects a byte array");
            };
            let ScriptArrayStorage::U8(bytes) = vm.bx.heap.array_storage(array) else {
                return script_err_type_mismatch!(vm.trap(), "woff2.decompress expects a U8 byte array");
            };
            let bytes = bytes.clone();
            match makepad_woff2::decompress(&bytes) {
                Some(sfnt) => vm.bx.heap.new_array_from_vec_u8(sfnt).into(),
                None => NIL,
            }
        },
    );
}
