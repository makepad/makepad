//! What `ColumnPicker`, `TreeSelect` and `Transfer` share, and nothing else.
//!
//! A drop-down works while the answers fit on one screen and the reader
//! knows which one they want. Past that it stops working, and those three
//! controls are the three shapes that replace it: a path through a
//! hierarchy, a set out of a hierarchy, and what is in against what is out.
//! Each has a file of its own. This one holds what more than one of them
//! needs — the flattened forest the two outline-driven ones are built on,
//! the text elider, and the panel and row surfaces all three draw with — so
//! that none of them reaches into another's file for it.
//!
//! # What the three deliberately do NOT do
//!
//! * **No scrolling of their own.** Every row is drawn every pass and rows
//!   past the stated budget are not drawn at all. That is honest for a
//!   picker of a few dozen rows, which is what all three are for; past that
//!   the answer is a list widget with a viewport, not a scrollbar bolted on
//!   here.
//! * **No data source, no lazy children.** A host hands over the whole set
//!   up front, as an indented outline or a flat list of words, and gets back
//!   typed actions. A column picker that fetched the next column would need
//!   a loading state, a failure state and a cancel, and none of those are
//!   decisions a widget gets to make.
//! * **No sorting and no grouping.** The order given is the order shown.
//!
//! The look decisions worth writing down: every mark in the three is a
//! rectangle, a rounded box, a circle or a TEXT GLYPH. Small marks drawn as
//! shader paths do not paint reliably in this renderer, so there is not one
//! in any of them. And the glyphs used are all ones the default face carries
//! — the angle quotes and the guillemets — because a missing glyph draws as
//! an empty box and nothing says so.

use crate::{badge::measure, makepad_draw::*};

/// Where a glyph's ink begins below the y handed to `draw_abs`, as a share
/// of the font size: the call takes the top of the LINE box, not the ink.
const INK_TOP: f64 = 0.30;

/// The y to hand `draw_abs` so one line of `font_size` sits centred in a box
/// of `height` starting at `top`. `draw_abs` takes the top of the line box
/// and the ink starts about [`INK_TOP`] of the font size below it, so a run
/// centred on the line box alone rides low by that much.
pub(crate) fn text_y(font_size: f64, top: f64, height: f64) -> f64 {
    top + (height - font_size) * 0.5 - font_size * INK_TOP
}

/// `text` cut to fit `max_w`, with an ellipsis when anything came off.
///
/// The first cut is guessed from the ratio of the measured width to the room
/// available, so a label twice too long costs two or three measurements
/// rather than one per character taken off.
pub(crate) fn elide(draw_text: &DrawText, cx: &mut Cx2d, text: &str, max_w: f64) -> String {
    if max_w <= 0.0 {
        return String::new();
    }
    let full = measure(draw_text, cx, text);
    if full <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = ((chars.len() as f64) * (max_w / full.max(1.0))).floor() as usize;
    keep = keep.min(chars.len());
    loop {
        if keep == 0 {
            return String::new();
        }
        let mut cut: String = chars[..keep].iter().collect();
        cut.push('\u{2026}');
        if measure(draw_text, cx, &cut) <= max_w {
            return cut;
        }
        keep -= 1;
    }
}

/// How deep an outline line is indented: a tab, or every two spaces, is one
/// level. An odd trailing space is ignored rather than rounded up.
fn indent_of(line: &str) -> usize {
    let mut level = 0;
    let mut spaces = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces == 2 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

/// One node of the forest the two outline-driven pickers share.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) id: LiveId,
    pub(crate) label: String,
    pub(crate) depth: usize,
    pub(crate) children: Vec<usize>,
}

/// A nested set of names, flattened once into document order.
///
/// It is deliberately smaller than a general tree: no fold state, no icons,
/// no selection, no host-supplied ids. Neither control built on it needs any
/// of those, and a picker carrying a tree's whole state would give two
/// places to look for the answer to "what is chosen".
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Forest {
    pub(crate) nodes: Vec<Node>,
    pub(crate) roots: Vec<usize>,
}

impl Forest {
    /// A forest written as indented lines. Ids are the line numbers counting
    /// from one, so a host reading an action back can find its own line.
    ///
    /// An indent that jumps more than one level past the line above it is
    /// clamped to one level rather than refused: a hand-written outline with
    /// a stray space should still draw the shape its author meant.
    pub(crate) fn from_outline(lines: &[String]) -> Self {
        let mut forest = Self::default();
        // The node standing at each depth, so a line's parent is whatever is
        // on top once the stack is cut back to that line's own depth.
        let mut stack: Vec<usize> = Vec::new();
        for (line_no, line) in lines.iter().enumerate() {
            let label = line.trim();
            if label.is_empty() {
                continue;
            }
            let depth = indent_of(line).min(stack.len());
            stack.truncate(depth);
            let index = forest.nodes.len();
            forest.nodes.push(Node {
                id: LiveId(line_no as u64 + 1),
                label: label.to_string(),
                depth,
                children: Vec::new(),
            });
            match stack.last() {
                Some(&parent) => forest.nodes[parent].children.push(index),
                None => forest.roots.push(index),
            }
            stack.push(index);
        }
        forest
    }

    pub(crate) fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    pub(crate) fn children(&self, index: usize) -> &[usize] {
        self.nodes.get(index).map(|node| node.children.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn is_leaf(&self, index: usize) -> bool {
        self.children(index).is_empty()
    }

    pub(crate) fn label(&self, index: usize) -> &str {
        self.nodes.get(index).map(|node| node.label.as_str()).unwrap_or("")
    }

    pub(crate) fn id(&self, index: usize) -> Option<LiveId> {
        self.nodes.get(index).map(|node| node.id)
    }

    pub(crate) fn index_of(&self, id: LiveId) -> Option<usize> {
        self.nodes.iter().position(|node| node.id == id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The shared surfaces. Every value with a Rust field behind it is
    // written PLAIN and every value the shader alone owns is a uniform:
    // a plain value with no field behind it would take an instance slot and
    // push the fields that do have one off their own, and the row would then
    // read a colour channel as its hover.
    set_type_default() do #(DrawPickerPanel::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: uniform(#00000000)
        border_color: uniform(#00000000)
        /** the outline's thickness in pixels 0..4 step 0.5 */
        border_size: uniform(1.0)
        /** corner rounding radius 0..16 step 0.5 */
        radius: uniform(theme.corner_radius)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(
                self.border_size * 0.5
                self.border_size * 0.5
                self.rect_size.x - self.border_size
                self.rect_size.y - self.border_size
                self.radius
            )
            // fill_KEEP: a plain fill wipes the shape and the stroke that
            // follows lands on nothing.
            sdf.fill_keep(self.color)
            sdf.stroke(self.border_color, self.border_size)
            return sdf.result
        }
    }

    set_type_default() do #(DrawPickerRow::script_shader(vm)){
        ..mod.draw.DrawQuad
        hover: 0.0
        chosen: 0.0
        keyed: 0.0
        color: uniform(#00000000)
        color_hover: uniform(theme.color_surface_container_high)
        color_chosen: uniform(theme.color_primary_container)
        /** the ring marking where the arrow keys are standing */
        color_ring: uniform(theme.color_primary)
        /** how thick the keyboard ring is 0..3 step 0.5 */
        ring_size: uniform(1.0)
        /** corner rounding radius 0..12 step 0.5 */
        radius: uniform(3.0)
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, self.radius)
            // Being chosen is a FILL and being where the keys are is a RING.
            // They are different facts and a reader has to be able to see
            // both at once: a keyed row that borrowed the chosen fill would
            // be claiming a choice nobody has made yet.
            sdf.fill_keep(self.color.mix(self.color_hover, self.hover).mix(self.color_chosen, self.chosen))
            sdf.stroke(vec4(0.0, 0.0, 0.0, 0.0).mix(self.color_ring, self.keyed), self.ring_size)
            return sdf.result
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerPanel {
    #[deref]
    draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawPickerRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub hover: f32,
    #[live]
    pub chosen: f32,
    #[live]
    pub keyed: f32,
}

/// The outlines the pickers' own tests are written against, kept beside the
/// forest so neither widget's tests have to borrow the other's.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::Forest;

    pub(crate) fn forest(lines: &[&str]) -> Forest {
        let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
        Forest::from_outline(&lines)
    }

    /// Four levels: continent, country, region, city.
    pub(crate) fn places() -> Forest {
        forest(&[
            "Europe",
            "  France",
            "    Brittany",
            "      Rennes",
            "      Brest",
            "    Alsace",
            "      Colmar",
            "  Spain",
            "    Galicia",
            "      Vigo",
            "Asia",
            "  Japan",
            "    Kansai",
            "      Kobe",
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::forest;
    use super::*;

    #[test]
    fn an_outline_indent_that_jumps_is_clamped_rather_than_refused() {
        // A hand-written outline with a stray extra indent should still draw
        // the shape its author meant, not lose the line.
        let forest = forest(&["Top", "      Deep"]);
        assert_eq!(forest.nodes.len(), 2);
        assert_eq!(forest.nodes[1].depth, 1);
        assert_eq!(forest.children(0), [1usize].as_slice());
    }

    #[test]
    fn an_outline_line_carries_its_own_id_so_a_host_can_find_it_again() {
        // Ids are line numbers counting from one, blank lines included, so
        // an action read back names the line the host actually wrote.
        let forest = forest(&["Europe", "", "  France"]);
        assert_eq!(forest.nodes.len(), 2);
        assert_eq!(forest.id(0), Some(LiveId(1)));
        assert_eq!(forest.id(1), Some(LiveId(3)));
        assert_eq!(forest.index_of(LiveId(3)), Some(1));
    }

    #[test]
    fn the_shared_surfaces_register_before_the_three_that_draw_with_them() {
        // The three merge their colours into a panel and a row whose type
        // defaults are the shaders registered here, so those have to be
        // standing first. The order is only a list of calls in lib.rs, and
        // nothing else says when it has been sorted.
        let lib = include_str!("lib.rs");
        let at = |module: &str| {
            lib.find(&format!("crate::{module}::script_mod(vm);"))
                .unwrap_or_else(|| panic!("{module} is not registered"))
        };
        let parts = at("picker_parts");
        for user in ["column_picker", "tree_select", "transfer"] {
            assert!(parts < at(user), "{user} registers before the surfaces it draws with");
        }
    }
}
