//! The coverage page: every declaration under `mod.widgets`, whether a story
//! shows it, and where it sits on the style ladder.
//!
//! The library's real organising axis is the ladder: `<Name>Flat` carries the
//! geometry and shaders, the bare name adds the bevel, `GradientX`/`GradientY`
//! add a fill gradient, `Flatter` is the ghost, `Icon` the icon-only face.
//! Walking the widget module through the script heap lists every rung of
//! every family, so the catalogue can say what it is not showing yet instead
//! of leaving that to memory. The walk happens once, on the first draw, and
//! the count goes to the log so a test can hold it.
use crate::makepad_widgets::*;
use crate::registry::{self, Story};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    let CoverageRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        padding: Inset{top: 2. bottom: 2. left: 0. right: 0.}
        name := Label{width: 240. text: ""}
        kind := Label{width: 140. text: ""}
        base := Label{width: 200. text: ""}
        rung := Label{width: 90. text: ""}
        covered := Label{width: 90. text: ""}
    }

    let CoverageHead = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        padding: Inset{top: 2. bottom: 6. left: 0. right: 0.}
        Labelbold{width: 240. text: "declaration"}
        Labelbold{width: 140. text: "rust type"}
        Labelbold{width: 200. text: "derives from"}
        Labelbold{width: 90. text: "rung"}
        Labelbold{width: 90. text: "story"}
    }

    mod.widgets.StoryCoverageBase = #(StoryCoverage::register_widget(vm))
    mod.widgets.StoryCoverage = set_type_default() do mod.widgets.StoryCoverageBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: theme.space_2
        summary := Label{text: ""}
        list := PortalList{
            width: Fill
            height: Fill
            scroll_bar: ScrollBar{}
            Head := CoverageHead{}
            Row := CoverageRow{}
        }
    }

    mod.stories.Coverage = StoryPage{
        mod.widgets.StoryCoverage{}
    }
}

pub const STORIES: &[Story] = &[Story {
    key: "overview/coverage/coverage",
    category: "Overview",
    component: "Coverage",
    name: "Coverage",
    dsl: "Coverage",
    added: "2025-06-01",
    tags: &["registry", "ladder"],
    doc: "# Coverage\n\nEvery declaration under the widget module, the Rust type behind it, what it derives from, its rung on the style ladder, and whether a story shows its family. The summary line is also written to the log.",
    subject: "",
    feature: None,
    controls: &[],
    on_actions: None,
}];

/// One declaration under the widget module.
#[derive(Clone)]
pub struct Declaration {
    pub name: String,
    pub kind: String,
    pub base: String,
    pub rung: String,
    pub covered: bool,
    /// Whether a story could show this at all. A `Draw*` shader struct and a
    /// bare enum are declared under `mod.widgets` like everything else and
    /// can never be rendered on a page of their own; counting them as
    /// uncovered put several hundred pages into a denominator that no amount
    /// of work could ever close.
    pub storyable: bool,
}

const RUNGS: &[&str] = &["GradientX", "GradientY", "Flatter", "Flat", "Icon", "Base"];

/// The ladder rung a name sits on, from its suffix.
pub fn rung_of(name: &str) -> &'static str {
    for rung in RUNGS {
        if name.ends_with(rung) {
            return rung;
        }
    }
    "standard"
}

/// The family a name belongs to: the name with every rung suffix stripped.
pub fn family_of(name: &str) -> String {
    let mut s = name.to_string();
    loop {
        let before = s.len();
        for rung in RUNGS {
            if s.len() > rung.len() && s.ends_with(rung) {
                s.truncate(s.len() - rung.len());
            }
        }
        if s.len() == before {
            return s;
        }
    }
}

fn id_name(id: LiveId) -> String {
    // The fallback formats outside the interning table's callback: formatting
    // an id looks the table up again, and doing that inside the callback
    // stalls on the table's own lock.
    let named = id.as_string(|s| s.map(|s| s.to_string()));
    named.unwrap_or_else(|| format!("{}", id))
}

#[derive(Script, ScriptHook, Widget)]
pub struct StoryCoverage {
    #[deref]
    view: View,
    #[rust]
    rows: Vec<Declaration>,
    #[rust]
    built: bool,
}

impl StoryCoverage {
    fn build(&mut self, cx: &mut Cx) {
        let mut rows: Vec<Declaration> = Vec::new();
        // The registry knows exactly which Rust types can be built as a
        // widget, which is the only honest test for "could a story show
        // this". Asking it beats keeping a list of exceptions in step.
        let widget_types: std::collections::BTreeSet<String> = cx
            .components
            .get::<WidgetRegistry>()
            .map
            .values()
            .map(|(info, _)| id_name(info.name))
            .collect();
        cx.with_vm(|vm| {
            let widgets = vm.module(id!(widgets));
            let top: Vec<(String, ScriptValue)> = vm.map_mut_with(widgets, |_vm, map| {
                map.iter()
                    .filter_map(|(k, v)| k.as_id().map(|id| (id_name(id), v.value)))
                    .collect()
            });
            let mut entries: Vec<(String, ScriptValue)> = Vec::new();
            for (name, value) in top {
                // A namespace is an object with no Rust type of its own whose
                // members are typed objects: `mod.widgets.glass`, for one.
                let namespace = match value.as_object() {
                    Some(obj) if vm.bx.heap.object_type_id(obj).is_none() => {
                        vm.map_mut_with(obj, |vm, map| {
                            map.iter().any(|(_, v)| {
                                v.value
                                    .as_object()
                                    .and_then(|o| vm.bx.heap.object_type_id(o))
                                    .is_some()
                            })
                        })
                    }
                    _ => false,
                };
                if namespace {
                    let obj = value.as_object().unwrap();
                    let inner: Vec<(String, ScriptValue)> = vm.map_mut_with(obj, |_vm, map| {
                        map.iter()
                            .filter_map(|(k, v)| {
                                k.as_id().map(|id| (format!("{}.{}", name, id_name(id)), v.value))
                            })
                            .collect()
                    });
                    entries.extend(inner);
                } else {
                    entries.push((name, value));
                }
            }
            let objects: Vec<(String, ScriptObject)> = entries
                .iter()
                .filter_map(|(n, v)| v.as_object().map(|o| (n.clone(), o)))
                .collect();
            for (name, value) in &entries {
                let Some(obj) = value.as_object() else {
                    continue;
                };
                // Walk the prototypes by hand: the construction chain also
                // resolves every level's source docs, which is far too slow
                // across four hundred declarations.
                let mut kind = None;
                let mut cur = Some(obj);
                let mut depth = 0;
                while let Some(o) = cur {
                    if let Some(t) = vm.bx.heap.object_type_id(o) {
                        kind = vm.bx.heap.type_name_by_id(t).map(id_name);
                        break;
                    }
                    depth += 1;
                    if depth > 32 {
                        break;
                    }
                    cur = vm.bx.heap.proto(o).as_object();
                }
                let kind = kind.unwrap_or_else(|| "template".to_string());
                let base = vm
                    .bx
                    .heap
                    .proto(obj)
                    .as_object()
                    .and_then(|p| objects.iter().find(|(n, o)| *o == p && n != name))
                    .map(|(n, _)| n.clone())
                    .unwrap_or_default();
                let short = name.rsplit('.').next().unwrap_or(name);
                let family = family_of(short);
                let covered =
                    registry::all().any(|s| s.component == family || s.component == short);
                let storyable = widget_types.contains(&kind);
                rows.push(Declaration {
                    name: name.clone(),
                    kind,
                    base,
                    rung: rung_of(short).to_string(),
                    covered,
                    storyable,
                });
            }
        });
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let storyable_rows: Vec<&Declaration> = rows.iter().filter(|r| r.storyable).collect();
        let shown = storyable_rows.iter().filter(|r| r.covered).count();
        let total = storyable_rows.len();
        let other = rows.len() - total;
        let families: std::collections::BTreeSet<String> = storyable_rows
            .iter()
            .map(|r| family_of(r.name.rsplit('.').next().unwrap_or(&r.name)))
            .collect();
        let summary = format!(
            "{} of {} declarations a story could show have one, in {} families \u{2014} and {} more no story could show: shaders, enums and the property types the DSL names",
            shown,
            total,
            families.len(),
            other
        );
        log!(
            "storybook: coverage {} of {} storyable declarations shown ({} not storyable)",
            shown,
            total,
            other
        );
        self.view.label(cx, ids!(summary)).set_text(cx, &summary);
        self.rows = rows;
        self.built = true;
    }
}

impl Widget for StoryCoverage {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.built {
            self.build(cx);
        }
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                list.set_item_range(cx, 0, self.rows.len() + 1);
                while let Some(item_id) = list.next_visible_item(cx) {
                    if item_id == 0 {
                        let head = list.item(cx, item_id, live_id!(Head));
                        head.draw_all(cx, &mut Scope::empty());
                        continue;
                    }
                    let Some(row) = self.rows.get(item_id - 1) else {
                        continue;
                    };
                    let item = list.item(cx, item_id, live_id!(Row));
                    item.label(cx, ids!(name)).set_text(cx, &row.name);
                    item.label(cx, ids!(kind)).set_text(cx, &row.kind);
                    item.label(cx, ids!(base)).set_text(cx, &row.base);
                    item.label(cx, ids!(rung)).set_text(cx, &row.rung);
                    item.label(cx, ids!(covered)).set_text(
                        cx,
                        match (row.storyable, row.covered) {
                            (false, _) => "-",
                            (true, true) => "yes",
                            (true, false) => "no",
                        },
                    );
                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if let Event::LiveEdit = event {
            self.built = false;
            self.rows.clear();
        }
        self.view.handle_event(cx, event, scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rungs_and_families() {
        assert_eq!(rung_of("ButtonFlat"), "Flat");
        assert_eq!(rung_of("ButtonGradientX"), "GradientX");
        assert_eq!(rung_of("Button"), "standard");
        assert_eq!(family_of("ButtonFlatIcon"), "Button");
        assert_eq!(family_of("ButtonGradientYIcon"), "Button");
        assert_eq!(family_of("Slider"), "Slider");
        assert_eq!(family_of("Flat"), "Flat");
    }
}
