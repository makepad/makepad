# The AI-edit corpus for examples/counter (see todo.py for the format).
PKG = "makepad-example-counter"
ARGS = ["--no-default-features"]
FILE = "examples/counter/src/main.rs"
EDITS = [
    ("label size", "style", "draw_text.text_style.font_size: 24", "draw_text.text_style.font_size: 30"),
    ("button text", "text", 'text: "Increment"', 'text: "Add one"'),
    ("window size", "layout", "window.inner_size: vec2(420, 220)", "window.inner_size: vec2(480, 260)"),
    ("spacing", "layout", "spacing: 12", "spacing: 20"),
    ("label prefix", "text", 'text: "Count: " + state.counter', 'text: "Clicks: " + state.counter'),
    ("step by two", "logic", "mod.state.counter += 1", "mod.state.counter += 2"),
    ("new button", "child widget", '                    increment_button := Button{\n                        text: "Add one"\n                    }', '                    increment_button := Button{\n                        text: "Add one"\n                    }\n                    reset_button := Button{\n                        text: "Reset"\n                    }'),
    ("reset handler", "logic", "    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {\n", "    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {\n        if self.ui.button(cx, ids!(reset_button)).clicked(actions) {\n            script_eval!(cx,{\n                mod.state.counter = 0\n                ui.main_view.render()\n            });\n        }\n"),
    ("app field", "field add", "pub struct App {\n    #[live]\n    ui: WidgetRef,\n}", "pub struct App {\n    #[live]\n    ui: WidgetRef,\n    #[rust]\n    clicks: u32,\n}"),
    ("count clicks", "logic", "        if self.ui.button(cx, ids!(increment_button)).clicked(actions) {\n", "        if self.ui.button(cx, ids!(increment_button)).clicked(actions) {\n            self.clicks += 1;\n"),
    ("helper fn", "new fn", "impl MatchEvent for App {", "fn step_for(clicks: u32) -> u32 {\n    if clicks > 10 { 5 } else { 2 }\n}\n\nimpl MatchEvent for App {"),
    ("log step", "logic", "            self.clicks += 1;\n", "            self.clicks += 1;\n            log!(\"step {}\", step_for(self.clicks));\n"),
    ("align", "layout", "align: Center\n", "align: Align{x: 0.5 y: 0.3}\n"),
    ("rename field", "field rename", [("    #[rust]\n    clicks: u32,", "    #[rust]\n    presses: u32,"),
        ("            self.clicks += 1;\n            log!(\"step {}\", step_for(self.clicks));", "            self.presses += 1;\n            log!(\"step {}\", step_for(self.presses));")]),
    ("retype field", "field retype", [("    presses: u32,", "    presses: u64,"), ("step_for(self.presses)", "step_for(self.presses as u32)")]),
    ("signature change", "signature", [("fn step_for(clicks: u32) -> u32 {\n    if clicks > 10 { 5 } else { 2 }", "fn step_for(clicks: u32, big: u32) -> u32 {\n    if clicks > 10 { big } else { 2 }"),
        ("step_for(self.presses as u32)", "step_for(self.presses as u32, 5)")]),
    ("new struct", "new type", "#[derive(Script, ScriptHook)]\npub struct App {", "#[derive(Clone, Copy, Default, Debug)]\nstruct Stats {\n    best: u64,\n}\n\n#[derive(Script, ScriptHook)]\npub struct App {"),
    ("struct field", "field add", "    #[rust]\n    presses: u64,", "    #[rust]\n    presses: u64,\n    #[rust]\n    stats: Stats,"),
    ("use struct", "logic", "            self.presses += 1;\n", "            self.presses += 1;\n            self.stats.best = self.stats.best.max(self.presses);\n"),
    ("typo", "typo", "            self.presses += 1;\n", "            self.presses += 1\n"),
    ("typo fixed", "typo", "            self.presses += 1\n", "            self.presses += 1;\n"),
    ("remove field", "field remove", [("    #[rust]\n    stats: Stats,", ""), ("            self.stats.best = self.stats.best.max(self.presses);\n", "")]),
    ("icon size", "style", "AppIcon{name: \"counter\" width: 48 height: 48}", "AppIcon{name: \"counter\" width: 64 height: 64}"),
]
