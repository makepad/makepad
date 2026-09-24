//! A design session: one file under design, driven from a running app.
//!
//! The session is what a builder UI talks to. It opens the file a picked
//! widget was declared in, turns gestures into operations over that widget's
//! literal, previews every change through hot reload, and, when the reload
//! lands, says which widget to select next or rolls the change back when
//! the runtime refused it. It never writes the source file: a patch is
//! written under `local/design/` for the person or the agent to apply.

use super::doc::{DesignDoc, PreviewOutcome};
use super::locate::{locate_widget, widget_source_file, NodeSpan};
use super::ops::{fresh_name, DesignOp, Placement};
use super::text;
use crate::makepad_draw::*;
use crate::view::View;
use crate::widget::*;
use crate::widget_tree::{live_id_token, CxWidgetExt};

/// Where an insert goes, relative to the selection.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Place {
    Before,
    #[default]
    Inside,
    After,
}

/// A structural operation on the selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Structural {
    Delete,
    Duplicate,
    /// Put the selection inside a new `View{flow: Down}`.
    Wrap,
    /// Swap with the previous sibling.
    Up,
    /// Swap with the next sibling.
    Down,
    /// Move out of its parent, right after it.
    Out,
}

/// What a landed preview reported.
pub struct Landed {
    /// The runtime's errors, when the change was rolled back.
    pub errors: Vec<String>,
    /// The path of the widget to select once the tree is rebuilt.
    pub select: Option<String>,
}

struct Landing {
    select: Option<String>,
}

/// One palette entry: the type to insert and the body it comes with.
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub name: String,
    pub body: String,
}

pub struct DesignSession {
    doc: DesignDoc,
    landing: Option<Landing>,
    /// One line for the panel: what is under design and what last happened.
    pub status: String,
}

impl DesignSession {
    /// Open a session on the file `widget` was declared in.
    pub fn open(cx: &mut Cx, widget: &WidgetRef) -> Result<Self, String> {
        let span = locate_widget(cx, widget, None)?;
        let doc = DesignDoc::from_text(&span.file, span.text);
        let mut session = Self { doc, landing: None, status: String::new() };
        session.status = session.status_line();
        Ok(session)
    }

    pub fn file(&self) -> &str {
        &self.doc.file
    }

    pub fn doc(&self) -> &DesignDoc {
        &self.doc
    }

    /// Whether a preview has been queued and its reload has not landed yet.
    pub fn is_landing(&self) -> bool {
        self.landing.is_some()
    }

    /// The panel's status line: the file, the edit count, the dirty mark.
    pub fn status_line(&self) -> String {
        let n = self.doc.hunks().len();
        format!(
            "{}: {} edit{}{}",
            short(&self.doc.file),
            n,
            if n == 1 { "" } else { "s" },
            if self.doc.is_dirty() { " (previewed, not written)" } else { "" }
        )
    }

    fn locate(&self, cx: &mut Cx, widget: &WidgetRef) -> Result<NodeSpan, String> {
        let file = widget_source_file(cx, widget)?;
        if file != self.doc.file {
            return Err(format!(
                "declared in {}, not in {} which is under design",
                short(&file),
                short(&self.doc.file)
            ));
        }
        locate_widget(cx, widget, Some(self.doc.text())).map_err(|err| {
            if self.landing.is_some() {
                "the preview is still landing; try again after the next frame".to_string()
            } else {
                err
            }
        })
    }

    /// The literal holding `span` and the index of `span` among its
    /// children.
    fn parent_of(span: &NodeSpan) -> Result<(NodeSpan, usize), String> {
        let block = &span.blocks[span.block];
        let (parent, index) =
            text::parent_of(&span.text, block.body_start..block.body_end, &span.node)
                .ok_or_else(|| "the literal has no parent in its block".to_string())?;
        Ok((NodeSpan { node: parent, ..span.clone() }, index))
    }

    /// Insert `ty` with `body` (e.g. `Button`, `{text: "Save"}`) relative to
    /// `widget`. The new widget is named `<ty>_<n>` so it can be addressed by
    /// path. Returns the path to select once the preview lands.
    pub fn insert(
        &mut self,
        cx: &mut Cx,
        widget: &WidgetRef,
        place: Place,
        ty: &str,
        body: &str,
    ) -> Result<(), String> {
        let span = self.locate(cx, widget)?;
        let path = path_of(cx, widget);
        let name = fresh_name(self.doc.text(), ty);
        let splash = format!("{} := {}{}", name, ty, body.trim());
        let (parent, placement, parent_path) = match place {
            Place::Inside => {
                if widget.borrow::<View>().is_none() {
                    return Err(format!(
                        "{} is not a container; insert before or after it",
                        span.node.ty
                    ));
                }
                (span, Placement::Last, path)
            }
            Place::Before | Place::After => {
                let (parent, index) = Self::parent_of(&span)?;
                let placement = if place == Place::Before {
                    Placement::Before(index)
                } else {
                    Placement::After(index)
                };
                (parent, placement, parent_path(&path))
            }
        };
        DesignOp::Insert { parent, placement, splash }.apply(&mut self.doc)?;
        let select = if parent_path.is_empty() { name } else { format!("{}.{}", parent_path, name) };
        self.preview(cx, Some(select))
    }

    /// Delete, duplicate, wrap or move the selection.
    pub fn structural(
        &mut self,
        cx: &mut Cx,
        widget: &WidgetRef,
        op: Structural,
    ) -> Result<(), String> {
        let span = self.locate(cx, widget)?;
        let path = path_of(cx, widget);
        let select = match op {
            Structural::Delete => {
                DesignOp::Delete { node: span }.apply(&mut self.doc)?;
                Some(parent_path(&path))
            }
            Structural::Duplicate => {
                DesignOp::Duplicate { node: span }.apply(&mut self.doc)?;
                Some(path)
            }
            Structural::Wrap => {
                DesignOp::Wrap { node: span, wrapper: "View{flow: Down}".to_string() }
                    .apply(&mut self.doc)?;
                Some(path)
            }
            Structural::Up | Structural::Down => {
                let (parent, index) = Self::parent_of(&span)?;
                let count = text::children(&span.text, &parent.node).len();
                // The placement indexes the siblings once the node is out.
                let placement = match op {
                    Structural::Up if index > 0 => Placement::Before(index - 1),
                    Structural::Down if index + 1 < count => Placement::After(index),
                    _ => return Err("already at that end".to_string()),
                };
                DesignOp::Move { node: span, parent, placement }.apply(&mut self.doc)?;
                Some(path)
            }
            Structural::Out => {
                let (parent, _) = Self::parent_of(&span)?;
                let (grand, parent_index) = Self::parent_of(&parent)?;
                let name = span.node.name().map(|n| n.to_string());
                DesignOp::Move { node: span, parent: grand, placement: Placement::After(parent_index) }
                    .apply(&mut self.doc)?;
                let grand_path = parent_path(&parent_path(&path));
                Some(match name {
                    Some(name) if !grand_path.is_empty() => format!("{}.{}", grand_path, name),
                    Some(name) => name,
                    None => grand_path,
                })
            }
        };
        self.preview(cx, select)
    }

    /// Write a property into the selection's literal (a dotted key descends
    /// into typed properties through `+:`).
    pub fn set_prop(
        &mut self,
        cx: &mut Cx,
        widget: &WidgetRef,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        let span = self.locate(cx, widget)?;
        let path = path_of(cx, widget);
        let merge = value.contains('{');
        DesignOp::SetProp { node: span, key: key.to_string(), value: value.to_string(), merge }
            .apply(&mut self.doc)?;
        self.preview(cx, Some(path))
    }

    pub fn can_undo(&self) -> bool {
        self.doc.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.doc.can_redo()
    }

    pub fn undo(&mut self, cx: &mut Cx) -> Result<(), String> {
        if !self.doc.undo() {
            return Err("nothing to undo".to_string());
        }
        self.preview(cx, None)
    }

    pub fn redo(&mut self, cx: &mut Cx) -> Result<(), String> {
        if !self.doc.redo() {
            return Err("nothing to redo".to_string());
        }
        self.preview(cx, None)
    }

    /// Back to the file as it was opened; the app runs its compiled code
    /// again.
    pub fn reset(&mut self, cx: &mut Cx) -> Result<(), String> {
        self.doc.reset();
        self.preview(cx, None)
    }

    /// Queue the working text for the running app and remember what to do
    /// when it lands. A refused text is undone on the spot.
    fn preview(&mut self, cx: &mut Cx, select: Option<String>) -> Result<(), String> {
        match self.doc.preview(cx) {
            PreviewOutcome::Refused(err) => {
                self.doc.undo();
                self.status = format!("refused: {}", err);
                Err(err)
            }
            PreviewOutcome::Unchanged => {
                self.status = self.status_line();
                Ok(())
            }
            PreviewOutcome::Queued | PreviewOutcome::Reverted => {
                self.landing = Some(Landing { select });
                self.status = self.status_line();
                cx.redraw_all();
                Ok(())
            }
        }
    }

    /// Called on `Event::LiveEdit`, after the tree has been rebuilt. Reads
    /// the errors the re-run raised; errors in the file under design roll
    /// the last edit back and queue the text before it.
    pub fn land(&mut self, cx: &mut Cx) -> Landed {
        let landing = self.landing.take();
        let errors = cx.take_live_edit_errors();
        let name = short(&self.doc.file);
        let mine: Vec<String> = errors.iter().filter(|e| e.contains(name)).cloned().collect();
        if !mine.is_empty() && landing.is_some() {
            let rolled_back = self.doc.undo();
            self.status = format!("rolled back: {}", mine[0]);
            if rolled_back {
                if let PreviewOutcome::Queued | PreviewOutcome::Reverted = self.doc.preview(cx) {
                    self.landing = Some(Landing { select: None });
                    cx.redraw_all();
                }
            }
            return Landed { errors: mine, select: None };
        }
        self.status = if errors.is_empty() {
            self.status_line()
        } else {
            format!("{} ({} error(s) elsewhere)", self.status_line(), errors.len())
        };
        Landed { errors: Vec::new(), select: landing.and_then(|l| l.select) }
    }

    /// Write the unified diff of the session to `local/design/<file>.patch`
    /// and return that path. The source file itself is never written.
    pub fn write_patch(&mut self) -> Result<String, String> {
        if !self.doc.is_dirty() {
            return Err("nothing to write: the file is as it was opened".to_string());
        }
        let dir = std::path::Path::new("local").join("design");
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
        let path = dir.join(format!("{}.patch", short(&self.doc.file)));
        let mut diff = self.doc.unified_diff();
        if !self.doc.base_matches_disk() {
            diff.insert_str(0, "# NOTE: the file on disk has changed since this session opened; apply with care\n");
        }
        std::fs::write(&path, diff).map_err(|e| format!("{}: {}", path.display(), e))?;
        let path = path.display().to_string();
        self.status = format!("patch written: {}", path);
        Ok(path)
    }
}

/// The widget's dotted path in the widget tree.
pub fn path_of(cx: &Cx, widget: &WidgetRef) -> String {
    let Some(uid) = widget.try_widget_uid() else {
        return String::new();
    };
    cx.widget_tree()
        .path_to(uid)
        .iter()
        .map(|id| live_id_token(*id))
        .collect::<Vec<_>>()
        .join(".")
}

fn parent_path(path: &str) -> String {
    match path.rfind('.') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn short(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

/// The palette: a curated set with a useful body first, then every other
/// typed entry of `mod.widgets` with an empty body.
pub fn palette(cx: &mut Cx) -> Vec<PaletteEntry> {
    const CURATED: &[(&str, &str)] = &[
        ("View", "{width: Fill height: Fit flow: Down spacing: 4 padding: 4}"),
        ("RoundedView", "{width: Fill height: 40 draw_bg +: {color: #x3a3a44}}"),
        ("ScrollYView", "{width: Fill height: Fill flow: Down}"),
        ("Label", "{text: \"Label\"}"),
        ("Button", "{text: \"Button\"}"),
        ("TextInput", "{width: 160 empty_text: \"Type here\"}"),
        ("CheckBox", "{text: \"Check\"}"),
        ("Slider", "{width: 160}"),
        ("Image", "{width: 64 height: 64}"),
        ("Hr", "{}"),
        ("Vr", "{}"),
        ("Filler", "{}"),
    ];
    let mut out: Vec<PaletteEntry> = CURATED
        .iter()
        .map(|(name, body)| PaletteEntry { name: name.to_string(), body: body.to_string() })
        .collect();
    let mut rest: Vec<String> = cx.with_vm(|vm| {
        let widgets = vm.module(id!(widgets));
        let top: Vec<(String, ScriptValue)> = vm.map_mut_with(widgets, |_vm, map| {
            map.iter()
                .filter_map(|(k, v)| k.as_id().map(|id| (live_id_token(id), v.value)))
                .collect()
        });
        let mut names = Vec::new();
        for (name, value) in top {
            let Some(obj) = value.as_object() else {
                continue;
            };
            // Typed somewhere up its prototypes: a widget, not a namespace
            // or a plain table.
            let mut typed = false;
            let mut cur = Some(obj);
            for _ in 0..32 {
                let Some(o) = cur else {
                    break;
                };
                if vm.bx.heap.object_type_id(o).is_some() {
                    typed = true;
                    break;
                }
                cur = vm.bx.heap.proto(o).as_object();
            }
            if typed && !name.ends_with("Base") && !CURATED.iter().any(|(n, _)| *n == name) {
                names.push(name);
            }
        }
        names
    });
    rest.sort();
    out.extend(rest.into_iter().map(|name| PaletteEntry { name, body: "{}".to_string() }));
    out
}
