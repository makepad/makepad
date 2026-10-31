# The AI-edit corpus for Amp: one cumulative UI-editing session (see todo.py).
# Each old string occurs exactly once after the preceding edits; grouped pairs
# are saved together. Only the adjacent typo/fix pair leaves invalid Rust.
PKG = "makepad-app-amp"
FILE = "apps/amp/src/main.rs"
EDITS = [
    ("window title", "text", 'window.title: "Makepad Amp"', 'window.title: "Amp — Music"'),
    ("window size", "layout", "window.inner_size: vec2(1360 900)", "window.inner_size: vec2(1440 920)"),
    ("chassis padding", "layout", "padding: Inset{left: 18 right: 18 top: 0 bottom: 16}", "padding: Inset{left: 20 right: 20 top: 0 bottom: 18}"),
    ("rail title", "text", 'align: Align{x: 0.5} text: "Makepad Amp"', 'align: Align{x: 0.5} text: "AMP / MUSIC"'),
    ("brand subtitle", "text", 'brand_sub := mod.widgets.HwCaption{text: "AMP"}', 'brand_sub := mod.widgets.HwCaption{text: "MUSIC PLAYER"}'),
    ("screen padding", "layout", "padding: Inset{left: 14 right: 14 top: 14 bottom: 14}", "padding: Inset{left: 12 right: 12 top: 12 bottom: 12}"),
    ("milk background", "style", "milk_frame := RectView{\n                                                width: Fill\n                                                height: Fill\n                                                draw_bg +: {color: #x000000}", "milk_frame := RectView{\n                                                width: Fill\n                                                height: Fill\n                                                draw_bg +: {color: #x080c12}"),
    ("preset drawer width", "layout", "width: 470\n                            height: Fill", "width: 490\n                            height: Fill"),
    ("preset heading", "text", 'mod.widgets.HwTitle{width: Fill text: "PRESETS"}', 'mod.widgets.HwTitle{width: Fill text: "VISUAL PRESETS"}'),
    ("preset hint", "child widget", "                            milk_well := mod.widgets.HwDisplay{", '                            preset_hint := Label{text: "Choose a preset for the music" draw_text.color: #x94a3b8}\n                            milk_well := mod.widgets.HwDisplay{'),
    ("hint color", "style", 'text: "Choose a preset for the music" draw_text.color: #x94a3b8', 'text: "Choose a preset for the music" draw_text.color: #xb8c4d4'),
    ("hint font size", "style", "preset_hint := Label{", "preset_hint := Label{draw_text.text_style.font_size: 11 "),
    ("playlist corners", "style", "radius: 4\n                                        flow: Down", "radius: 6\n                                        flow: Down"),
    ("meter height", "layout", "meter := mod.widgets.HwMeter{height: 56}", "meter := mod.widgets.HwMeter{height: 60}"),
    ("folder dialog title", "logic", 'set_title("Add a Folder of Music".into())', 'set_title("Choose Your Music Folder".into())'),
    ("folder request helper", "new fn", "impl App {\n", 'impl App {\n    fn record_folder_request(&mut self) {\n        log!("amp: opening music folder picker");\n    }\n\n'),
    ("call folder helper", "logic", '        if self.ui.button(cx, ids!(add_folder)).clicked(actions) {\n', '        if self.ui.button(cx, ids!(add_folder)).clicked(actions) {\n            self.record_folder_request();\n'),
    ("folder counter", "field add", "    ui: WidgetRef,", "    ui: WidgetRef,\n    #[rust]\n    folder_requests: u32,"),
    ("count folder requests", "logic", "    fn record_folder_request(&mut self) {\n", "    fn record_folder_request(&mut self) {\n        self.folder_requests = self.folder_requests.saturating_add(1);\n"),
    ("log request count", "logic", 'log!("amp: opening music folder picker");', 'log!("amp: opening music folder picker (request {})", self.folder_requests);'),
    ("rename request counter", "field rename", [
        ("    folder_requests: u32,", "    folder_picker_opens: u32,"),
        ("self.folder_requests = self.folder_requests.saturating_add(1);", "self.folder_picker_opens = self.folder_picker_opens.saturating_add(1);"),
        ('(request {})", self.folder_requests);', '(request {})", self.folder_picker_opens);'),
    ]),
    ("widen request counter", "field retype", "    folder_picker_opens: u32,", "    folder_picker_opens: u64,"),
    ("folder hint enum", "new type", "#[derive(Script, ScriptHook)]\npub struct App {", "#[derive(Clone, Copy, Debug)]\nenum FolderHint {\n    Ready,\n    Requested,\n}\n\n#[derive(Script, ScriptHook)]\npub struct App {"),
    ("folder hint field", "field add", "    folder_picker_opens: u64,", "    folder_picker_opens: u64,\n    #[rust(FolderHint::Ready)]\n    folder_hint: FolderHint,"),
    ("mark folder request", "logic", "    fn record_folder_request(&mut self) {\n", "    fn record_folder_request(&mut self) {\n        self.folder_hint = FolderHint::Requested;\n"),
    ("folder hint description", "new impl", "impl App {\n", 'impl FolderHint {\n    fn description(&self) -> &\'static str {\n        match self {\n            Self::Ready => "ready",\n            Self::Requested => "choosing a folder",\n        }\n    }\n}\n\nimpl App {\n'),
    ("append busy hint", "enum variant", [
        ("enum FolderHint {\n    Ready,\n    Requested,\n}", "enum FolderHint {\n    Ready,\n    Requested,\n    Busy,\n}"),
        ('            Self::Requested => "choosing a folder",', '            Self::Requested => "choosing a folder",\n            Self::Busy => "folder reader busy",'),
    ]),
    ("track busy reader", "logic", '                        self.model.error = Some("The folder reader is busy; try again".into());', '                        self.folder_hint = FolderHint::Busy;\n                        self.model.error = Some("The folder reader is busy; try again".into());'),
    ("log hint description", "logic", 'log!("amp: opening music folder picker (request {})", self.folder_picker_opens);', 'log!("amp: {} (request {})", self.folder_hint.description(), self.folder_picker_opens);'),
    ("helper source parameter", "signature", [
        ("fn record_folder_request(&mut self) {", "fn record_folder_request(&mut self, source: &str) {"),
        ('log!("amp: {} (request {})", self.folder_hint.description(), self.folder_picker_opens);', 'log!("amp: {} from {} (request {})", self.folder_hint.description(), source, self.folder_picker_opens);'),
        ("self.record_folder_request();", 'self.record_folder_request("add folder button");'),
    ]),
    ("missing assignment value", "typo", "self.folder_hint = FolderHint::Requested;", "self.folder_hint = ;"),
    ("fix assignment value", "typo", "self.folder_hint = ;", "self.folder_hint = FolderHint::Requested;"),
    ("remove request counter", "field remove", [
        ("    #[rust]\n    folder_picker_opens: u64,\n", ""),
        ("        self.folder_picker_opens = self.folder_picker_opens.saturating_add(1);\n", ""),
        ('log!("amp: {} from {} (request {})", self.folder_hint.description(), source, self.folder_picker_opens);', 'log!("amp: {} from {}", self.folder_hint.description(), source);'),
    ]),
    ("folder busy message", "logic", 'self.folder_hint = FolderHint::Busy;\n                        self.model.error = Some("The folder reader is busy; try again".into());', 'self.folder_hint = FolderHint::Busy;\n                        self.model.error = Some("The folder reader is busy; try adding the folder again shortly".into());'),
    ("preset hint wording", "text", 'text: "Choose a preset for the music"', 'text: "Choose a visual preset for your music"'),
    ("drawer padding", "layout", "padding: Inset{left: 22 right: 22 top: 20 bottom: 20}", "padding: Inset{left: 24 right: 24 top: 18 bottom: 18}"),
]
