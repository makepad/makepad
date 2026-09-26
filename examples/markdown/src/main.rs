//! Native Markdown rendering. Resources are supplied by the embedding app.
use makepad_widgets::*;
use makepad_markdown_widgets::view::MarkdownViewWidgetRefExt;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(820, 850)
                body +: {
                    ScrollYView {
                        width: Fill height: Fill flow: Down padding: 24
                        markdown := MarkdownView {width: Fill height: Fit}
                    }
                }
            }
        }
    }
}

const SOURCE: &str = r#"---
title: Native Markdown
---

# Native Markdown / 原生渲染

[TOC]

## Text and resources

**Bold**, *italic*, ~~strikethrough~~, `inline code`, :smiley: and a [link](https://makepad.nl).

![App-supplied image](sample.svg)

> The app supplies decoded resources. The renderer has no network or account access.

- [x] Unicode and emoji
- [x] Aligned tables
- [ ] Host navigation

| Content | Alignment |
| :--- | ---: |
| 中文 | 234 |
| **Markdown** | right |

## Code

```rust
fn main() {
    println!("Hello, 世界!");
}
```

```javascript
const hello = (name) => `Hello, ${name}!`;
```

## Mathematics

Inline $E=mc^2$ and a display formula:

$$\sum_{k=1}^{n} k = \frac{n(n+1)}{2}$$

## Diagrams

```mermaid
flowchart LR
    Source --> Parser --> NativeWidgets
```

```seq
Reader->App: Open article
App-->Reader: Rendered content
```

Reference links resolve across the entire document: [Makepad][framework].

[framework]: https://makepad.nl
"#;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="280" height="64"><rect width="280" height="64" rx="8" fill="#dceef0"/><circle cx="32" cy="32" r="18" fill="#217c80"/><path d="M24 32l6 6 12-14" fill="none" stroke="white" stroke-width="4"/></svg>"##;
        let image = makepad_markdown_widgets::content::prepare_image(svg).expect("embedded image");
        let mut images = std::collections::BTreeMap::new();
        images.insert("sample.svg".into(), image);
        let markdown = self.ui.markdown_view(cx, ids!(markdown));
        markdown.set_images(cx, std::sync::Arc::new(images));
        markdown.set_text(cx, SOURCE);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::theme_mod(vm);
        script_eval!(vm, {
            mod.theme = mod.themes.light
        });
        makepad_widgets::widgets_mod(vm);
        makepad_markdown_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
