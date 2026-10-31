# The AI-edit corpus for examples/todo (a list app): a cumulative vibe-coding session, one small
# edit at a time, the way an AI assistant edits a UI. (name, category, old, new): `old` must occur
# exactly once in the file as the previous edits left it.
PKG = "makepad-example-todo"
FILE = "examples/todo/src/main.rs"
EDITS = [
    ("title text", "text", 'text: "Todo 123"', 'text: "My Tasks"'),
    ("subtitle color", "style", 'draw_text.color: #x64748b\n                                draw_text.text_style.font_size: theme.font_size_p\n                            }\n                        }\n                    }',
     'draw_text.color: #x475569\n                                draw_text.text_style.font_size: theme.font_size_p\n                            }\n                        }\n                    }'),
    ("window size", "layout", "window.inner_size: vec2(520, 720)", "window.inner_size: vec2(560, 760)"),
    ("row padding", "layout", "padding: theme.mspace_2{left: theme.space_3, right: theme.space_3}\n        flow: Right spacing: theme.space_2", "padding: theme.mspace_3{left: theme.space_3, right: theme.space_3}\n        flow: Right spacing: theme.space_2"),
    ("row radius", "style", "draw_bg.border_radius: 8.0\n        draw_bg.border_size: 1.0", "draw_bg.border_radius: 12.0\n        draw_bg.border_size: 1.0"),
    ("status format", "logic", 'let label = format!("{remaining} remaining / {total} total");', 'let label = format!("{remaining} left of {total}");'),
    ("trim on add", "logic", "        let text = text.trim();\n        if text.is_empty() {", "        let text = text.trim().trim_end_matches('.');\n        if text.is_empty() {"),
    ("default tag", "logic", "            tag: String::new(),\n            done: false,", '            tag: "new".to_string(),\n            done: false,'),
    ("helper fn", "new fn", "impl App {\n", "fn tag_for(text: &str) -> String {\n    if text.contains('!') { \"urgent\".to_string() } else { \"new\".to_string() }\n}\n\nimpl App {\n"),
    ("use helper", "logic", '            tag: "new".to_string(),\n            done: false,', "            tag: tag_for(text),\n            done: false,"),
    ("add field priority", "field add", [("struct TodoItemData {\n    text: String,", "struct TodoItemData {\n    priority: u8,\n    text: String,"),
        ('    vec![TodoItemData {\n        text: "Get AI to control UI".to_string(),', '    vec![TodoItemData {\n        priority: 1,\n        text: "Get AI to control UI".to_string(),'),
        ("        TODOS.write().unwrap().push(TodoItemData {\n            text: text.to_string(),", "        TODOS.write().unwrap().push(TodoItemData {\n            priority: 0,\n            text: text.to_string(),")]),
    ("app counter field", "field add", "pub struct App {\n    #[live]\n    ui: WidgetRef,\n}", "pub struct App {\n    #[live]\n    ui: WidgetRef,\n    #[rust]\n    added: usize,\n}"),
    ("count adds", "logic", "        self.ui.text_input(cx, ids!(todo_input)).set_text(cx, \"\");", "        self.added += 1;\n        self.ui.text_input(cx, ids!(todo_input)).set_text(cx, \"\");"),
    ("status shows adds", "logic", 'let label = format!("{remaining} left of {total}");', 'let label = format!("{remaining} left of {total} ({} added)", self.added);'),
    ("new child label", "child widget", "                        Filler{}\n", "                        Filler{}\n                        hint := Label{text: \"tip: ! marks urgent\" draw_text.color: #x94a3b8}\n"),
    ("button color", "style", "                                    color: #x2563eb\n                                    color_hover: #x1d4ed8", "                                    color: #x16a34a\n                                    color_hover: #x15803d"),
    ("typo", "typo", "        self.added += 1;", "        self.added += 1\n"),
    ("typo fixed", "typo", "        self.added += 1\n", "        self.added += 1;"),
    ("rename field", "field rename", [("    #[rust]\n    added: usize,", "    #[rust]\n    added_count: usize,"), ("        self.added += 1;", "        self.added_count += 1;"), ('({} added)", self.added);', '({} added)", self.added_count);')]),
    ("retype priority", "field retype", "    priority: u8,", "    priority: u32,"),
    ("new enum", "new type", "#[derive(Clone, Debug)]\nstruct TodoItemData {", "#[derive(Clone, Copy, Debug, PartialEq)]\nenum Filter {\n    All,\n    Open,\n}\n\n#[derive(Clone, Debug)]\nstruct TodoItemData {"),
    ("filter field", "field add", "    #[rust]\n    added_count: usize,", "    #[rust]\n    added_count: usize,\n    #[rust(Filter::All)]\n    filter: Filter,"),
    ("enum variant add", "enum variant", "    All,\n    Open,\n}", "    All,\n    Open,\n    Done,\n}"),
    ("signature change", "signature", [("fn tag_for(text: &str) -> String {\n    if text.contains('!')", "fn tag_for(text: &str, urgent_mark: char) -> String {\n    if text.contains(urgent_mark)"), ("            tag: tag_for(text),", "            tag: tag_for(text, '!'),")]),
    ("remove field", "field remove", "    #[rust(Filter::All)]\n    filter: Filter,", ""),
    ("delete style", "style", "                color: #x64748b\n                color_hover: #x0f172a\n                text_style +: {font_size: theme.font_size_p}", "                color: #xdc2626\n                color_hover: #x991b1b\n                text_style +: {font_size: theme.font_size_p}"),
    ("new impl block", "new impl", "impl MatchEvent for App {", "impl TodoItemData {\n    fn is_urgent(&self) -> bool {\n        self.tag == \"urgent\"\n    }\n}\n\nimpl MatchEvent for App {"),
    ("use impl", "logic", "        let remaining = todos.iter().filter(|todo| !todo.done).count();", "        let remaining = todos.iter().filter(|todo| !todo.done).count();\n        let urgent = todos.iter().filter(|todo| todo.is_urgent()).count();"),
    ("status urgent", "logic", '({} added)", self.added_count);', '({} added, {urgent} urgent)", self.added_count);'),
]
