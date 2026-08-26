//! Mutable per-session scene state: selection, visibility, section, explode.
//!
//! Everything here is cheap to mutate and bumps `revision`. The renderer keeps
//! the last revision it uploaded its per-element lookup for and refreshes only
//! when it changes; the outliner/properties re-read on draw. No geometry is
//! ever modified by state changes.

use crate::model::bounds::{aabb_center, aabb_is_empty, aabb_radius};
use crate::model::ids::{ElementId, LayerId, StoryId};
use crate::model::scene::Scene;
use makepad_math::{Aabb, Plane, Vec3f};
use std::collections::HashSet;

#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub set: HashSet<ElementId>,
    /// The last-clicked element: the one the Properties editor shows and the
    /// one drawn with the brighter outline.
    pub active: Option<ElementId>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
    pub fn contains(&self, id: ElementId) -> bool {
        self.set.contains(&id)
    }
    pub fn len(&self) -> usize {
        self.set.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SectionPlane {
    /// Half-space to *keep* is `a*x + b*y + c*z + d >= 0`.
    pub plane: Plane,
    pub enabled: bool,
    /// Optional element the plane was placed from (for the UI to show).
    pub source: Option<ElementId>,
}

#[derive(Clone, Debug, Default)]
pub struct SectionState {
    pub enabled: bool,
    /// Up to 6 planes are honoured by the renderer; more are ignored.
    pub planes: Vec<SectionPlane>,
    /// Section box: keeps only what is inside. Combined with planes.
    pub boxed: Option<Aabb>,
    /// Draw the cut faces with a cap fill (vs. seeing inside hollow shells).
    pub caps: bool,
    /// Linear RGBA of the cap fill.
    pub cap_color: [f32; 4],
}

impl SectionPlane {
    /// Signed distance; positive is the half-space that survives the cut.
    #[inline]
    pub fn distance(&self, p: Vec3f) -> f32 {
        self.plane.a * p.x + self.plane.b * p.y + self.plane.c * p.z + self.plane.d
    }

    #[inline]
    pub fn keeps(&self, p: Vec3f) -> bool {
        !self.enabled || self.distance(p) >= 0.0
    }
}

impl SectionState {
    /// True when a world point survives every active cut — the exact test the
    /// mesh shader does, so a pick and the picture agree about what is there.
    pub fn keeps(&self, p: Vec3f) -> bool {
        if !self.enabled {
            return true;
        }
        for pl in &self.planes {
            if !pl.keeps(p) {
                return false;
            }
        }
        if let Some(b) = &self.boxed {
            if p.x < b.min.x
                || p.y < b.min.y
                || p.z < b.min.z
                || p.x > b.max.x
                || p.y > b.max.y
                || p.z > b.max.z
            {
                return false;
            }
        }
        true
    }

    /// True when nothing is cut away at all (fast path for pickers).
    pub fn is_inactive(&self) -> bool {
        !self.enabled || (self.planes.iter().all(|p| !p.enabled) && self.boxed.is_none())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExplodeMode {
    /// Stories slide apart along Z.
    #[default]
    ByStory,
    /// Every element moves away from the model center.
    ByElement,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExplodeState {
    /// 0 = assembled, 1 = fully exploded (one story height / bounds radius).
    pub amount: f32,
    pub mode: ExplodeMode,
}

impl ExplodeState {
    /// World-space offset applied to one element by the exploded view.
    ///
    /// **This is the only implementation.** The renderer's per-element lookup
    /// texture, the CPU picker and the measurement snapper all call it, so the
    /// picture and the picks can never disagree about where an element is.
    pub fn offset(&self, scene: &Scene, id: ElementId) -> Vec3f {
        if self.amount.abs() < 1e-4 {
            return Vec3f::default();
        }
        let Some(e) = scene.element(id) else {
            return Vec3f::default();
        };
        match self.mode {
            ExplodeMode::ByStory => {
                let Some(s) = e.story.and_then(|s| scene.stories.get(s.index())) else {
                    return Vec3f::default();
                };
                // Stories slide apart along Z, spaced by the tallest story gap
                // so nothing interpenetrates at amount = 1.
                let step = scene.story_spacing();
                let rank = scene.story_rank(s.id) as f32;
                Vec3f {
                    x: 0.0,
                    y: 0.0,
                    z: self.amount * rank * step,
                }
            }
            ExplodeMode::ByElement => {
                if aabb_is_empty(&e.bounds) || aabb_is_empty(&scene.bounds) {
                    return Vec3f::default();
                }
                let c = aabb_center(&scene.bounds);
                let d = aabb_center(&e.bounds) - c;
                let len = d.length();
                if len < 1e-5 {
                    return Vec3f::default();
                }
                d * (self.amount * aabb_radius(&scene.bounds) / len * 0.6)
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SceneState {
    pub selection: Selection,
    /// Explicitly hidden elements (H).
    pub hidden: HashSet<ElementId>,
    /// When `Some`, only these elements are visible (Shift+H isolate).
    pub isolated: Option<HashSet<ElementId>>,
    pub hidden_layers: HashSet<LayerId>,
    pub hidden_stories: HashSet<StoryId>,
    pub section: SectionState,
    pub explode: ExplodeState,
    /// Incremented on every mutation through the methods below.
    pub revision: u64,
}

impl SceneState {
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// True when the element itself is not hidden by any rule. Does not walk
    /// parents; the scene build already flattens story/layer onto elements.
    pub fn is_visible(&self, scene: &Scene, id: ElementId) -> bool {
        if self.hidden.contains(&id) {
            return false;
        }
        if let Some(iso) = &self.isolated {
            if !iso.contains(&id) {
                return false;
            }
        }
        if let Some(e) = scene.element(id) {
            if let Some(l) = e.layer {
                if self.hidden_layers.contains(&l) {
                    return false;
                }
            }
            if let Some(s) = e.story {
                if self.hidden_stories.contains(&s) {
                    return false;
                }
            }
        }
        true
    }

    /// One `true` per element, indexed by `ElementId.0`. Build it once per
    /// pick / per frame instead of hashing per triangle: the renderer feeds it
    /// straight into the per-element lookup texture and the picker indexes it.
    pub fn visibility_mask(&self, scene: &Scene) -> Vec<bool> {
        let mut mask = vec![true; scene.elements.len()];
        if self.everything_visible() {
            return mask;
        }
        for (i, m) in mask.iter_mut().enumerate() {
            *m = self.is_visible(scene, ElementId::from_index(i));
        }
        mask
    }

    /// True when no rule hides anything — the hover path's fast exit.
    pub fn everything_visible(&self) -> bool {
        self.hidden.is_empty()
            && self.isolated.is_none()
            && self.hidden_layers.is_empty()
            && self.hidden_stories.is_empty()
    }

    /// Shorthand for [`ExplodeState::offset`] — the single implementation.
    pub fn explode_offset(&self, scene: &Scene, id: ElementId) -> Vec3f {
        self.explode.offset(scene, id)
    }

    /// The per-element lookup the renderer uploads once per [`Self::revision`],
    /// in the layout frozen in `libs/fab/src/api.rs` §"Rendering seams"
    /// (blocker B1): **2 RGBA texels per element**, `elements.len() * 8`
    /// floats, linear in `element * 2 + texel` order.
    ///
    /// | texel | x | y | z | w |
    /// |---|---|---|---|---|
    /// | 0 | visibility 0/1 | selection: 0 none, 1 selected, 2 active | hover 0/1 | 0 |
    /// | 1 | explode offset x | y | z | 0 |
    ///
    /// Indexed by the vertex element id ([`crate::Vertex::element`]). Padding
    /// the tail out to whole `ELEMENT_LUT_WIDTH` rows is the uploader's job;
    /// what matters here is that the numbers come from the same place the
    /// picker reads them, so the GPU and the mouse cannot disagree about what
    /// is hidden, selected or displaced.
    pub fn element_lookup(&self, scene: &Scene, hover: Option<ElementId>, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(scene.elements.len() * 8);
        let all_visible = self.everything_visible();
        for i in 0..scene.elements.len() {
            let id = ElementId::from_index(i);
            let visible = all_visible || self.is_visible(scene, id);
            let selection = if self.selection.active == Some(id) {
                2.0
            } else if self.selection.contains(id) {
                1.0
            } else {
                0.0
            };
            let o = self.explode.offset(scene, id);
            out.extend_from_slice(&[
                if visible { 1.0 } else { 0.0 },
                selection,
                if hover == Some(id) { 1.0 } else { 0.0 },
                0.0,
                o.x,
                o.y,
                o.z,
                0.0,
            ]);
        }
    }

    pub fn select_only(&mut self, id: ElementId) {
        self.selection.set.clear();
        self.selection.set.insert(id);
        self.selection.active = Some(id);
        self.bump();
    }

    pub fn select_add(&mut self, id: ElementId) {
        self.selection.set.insert(id);
        self.selection.active = Some(id);
        self.bump();
    }

    pub fn select_toggle(&mut self, id: ElementId) {
        if self.selection.set.remove(&id) {
            if self.selection.active == Some(id) {
                self.selection.active = self.selection.set.iter().next().copied();
            }
        } else {
            self.selection.set.insert(id);
            self.selection.active = Some(id);
        }
        self.bump();
    }

    pub fn select_set(&mut self, ids: impl IntoIterator<Item = ElementId>) {
        self.selection.set = ids.into_iter().collect();
        self.selection.active = self.selection.set.iter().next().copied();
        self.bump();
    }

    pub fn clear_selection(&mut self) {
        if !self.selection.set.is_empty() || self.selection.active.is_some() {
            self.selection.set.clear();
            self.selection.active = None;
            self.bump();
        }
    }

    /// H — hide the selection.
    pub fn hide_selected(&mut self) {
        for id in self.selection.set.drain() {
            self.hidden.insert(id);
        }
        self.selection.active = None;
        self.bump();
    }

    /// Alt+H — unhide everything, drop isolation.
    pub fn unhide_all(&mut self) {
        self.hidden.clear();
        self.isolated = None;
        self.bump();
    }

    /// Shift+H — isolate the selection (hide everything else).
    pub fn isolate_selected(&mut self) {
        if self.selection.set.is_empty() {
            return;
        }
        self.isolated = Some(self.selection.set.clone());
        self.bump();
    }

    pub fn set_hidden(&mut self, id: ElementId, hidden: bool) {
        if hidden {
            self.hidden.insert(id);
        } else {
            self.hidden.remove(&id);
        }
        self.bump();
    }

    pub fn set_layer_hidden(&mut self, id: LayerId, hidden: bool) {
        if hidden {
            self.hidden_layers.insert(id);
        } else {
            self.hidden_layers.remove(&id);
        }
        self.bump();
    }

    pub fn set_story_hidden(&mut self, id: StoryId, hidden: bool) {
        if hidden {
            self.hidden_stories.insert(id);
        } else {
            self.hidden_stories.remove(&id);
        }
        self.bump();
    }

    pub fn set_section(&mut self, section: SectionState) {
        self.section = section;
        self.bump();
    }

    pub fn set_explode(&mut self, explode: ExplodeState) {
        if self.explode != explode {
            self.explode = explode;
            self.bump();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::scene::Scene;

    /// The GPU lookup and the CPU picker must never disagree about what is
    /// hidden, what is selected, or where an element has been displaced —
    /// that is the whole point of there being one `ExplodeState::offset`.
    #[test]
    fn the_lookup_says_what_the_picker_sees() {
        let scene = Scene::from_model(crate::model::demo::demo_house(), &mut |_| {});
        let mut state = SceneState::default();
        let first = scene
            .elements
            .iter()
            .find(|e| e.has_geometry())
            .map(|e| e.id)
            .unwrap();
        let last = scene
            .elements
            .iter()
            .rev()
            .find(|e| e.has_geometry() && e.id != first)
            .map(|e| e.id)
            .unwrap();
        state.select_only(first);
        state.select_add(last);
        state.set_hidden(first, true);
        state.set_explode(ExplodeState {
            amount: 0.7,
            mode: ExplodeMode::ByStory,
        });

        let mut lut = Vec::new();
        state.element_lookup(&scene, Some(last), &mut lut);
        assert_eq!(lut.len(), scene.elements.len() * 8);
        let mask = state.visibility_mask(&scene);
        for (i, e) in scene.elements.iter().enumerate() {
            let t = &lut[i * 8..i * 8 + 8];
            assert_eq!(t[0] > 0.5, mask[i], "visibility disagrees on {}", e.name);
            assert_eq!(t[0] > 0.5, state.is_visible(&scene, e.id));
            let o = state.explode.offset(&scene, e.id);
            assert_eq!([t[4], t[5], t[6]], [o.x, o.y, o.z], "offset disagrees");
        }
        // selection column: 2 = active, 1 = selected, 0 = neither
        assert_eq!(lut[last.index() * 8 + 1], 2.0);
        assert_eq!(lut[last.index() * 8 + 2], 1.0, "hover column");
        // The eye toggle hides without deselecting (only H deselects), so the
        // hidden element keeps its selection bit and loses its visibility bit.
        assert_eq!(lut[first.index() * 8 + 1], 1.0, "selection");
        assert_eq!(lut[first.index() * 8], 0.0, "visibility");
    }

    #[test]
    fn sections_keep_the_right_half_space() {
        use makepad_math::Plane;
        let mut s = SectionState {
            enabled: true,
            planes: vec![SectionPlane {
                // keep z >= 0
                plane: Plane {
                    a: 0.0,
                    b: 0.0,
                    c: 1.0,
                    d: 0.0,
                },
                enabled: true,
                source: None,
            }],
            boxed: None,
            caps: true,
            cap_color: [0.5; 4],
        };
        let up = Vec3f { x: 0.0, y: 0.0, z: 1.0 };
        let down = Vec3f { x: 0.0, y: 0.0, z: -1.0 };
        assert!(s.keeps(up) && !s.keeps(down));
        assert!(!s.is_inactive());
        s.enabled = false;
        assert!(s.keeps(down) && s.is_inactive());
        s.enabled = true;
        s.planes[0].enabled = false;
        assert!(s.keeps(down) && s.is_inactive());
        // A box keeps only what is inside it.
        s.boxed = Some(Aabb {
            min: Vec3f { x: -1.0, y: -1.0, z: -1.0 },
            max: Vec3f { x: 1.0, y: 1.0, z: 1.0 },
        });
        assert!(!s.is_inactive());
        assert!(s.keeps(down));
        assert!(!s.keeps(Vec3f { x: 0.0, y: 0.0, z: 9.0 }));
    }
}
