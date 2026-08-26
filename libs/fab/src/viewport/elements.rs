//! Lane B. The per-element lookup texture — how hide / isolate / select /
//! explode reach the GPU without touching a single vertex.
//!
//! ## Layout (the contract every consumer reads)
//!
//! * **Vertex lane.** The element index rides the `GameMeshVertexAo.ao_uv`
//!   slot (`viewport::pack::pack_element`): unorm16x2, low half in the first
//!   axis, so 2³² elements fit. Fab models carry no lightmap chart, which is
//!   what makes that lane free — no new POD, no change to
//!   `MODEL_VERTEX_FLOATS`, no second vertex format in `libs/render`.
//! * **Lookup texture.** `VecRGBAf32`, `LUT_WIDTH` texels per row, ONE texel
//!   per element, row-major by element index:
//!   | channel | meaning |
//!   |---|---|
//!   | `r` | state: `0` hidden, `1` visible, `2` selected, `3` active |
//!   | `g b a` | model-space (render, Y-up) offset added to every vertex |
//! * **Uniform.** `elem_ctl = (width, height, 0, enabled)` on the static
//!   shader; `0` in `w` disables the whole lookup.
//!
//! The static-mesh shader reads `r < 0.5` as "collapse this vertex to the
//! model origin", which makes the whole triangle zero-area — a hidden
//! element costs no fragments. The composite pass reads the same texture for
//! the selection outline, so the lit image and the outline can never
//! disagree about what is selected.
//!
//! Rebuilt only when `SceneState::revision` changes — never per frame
//! (Metal upload hazard, `local/agent_state/vj-transport-redesign/`).

use crate::api::*;
use crate::viewport::pack::to_render;
use makepad_widgets::*;

pub const LUT_WIDTH: usize = 256;

pub const STATE_HIDDEN: f32 = 0.0;
pub const STATE_VISIBLE: f32 = 1.0;
pub const STATE_SELECTED: f32 = 2.0;
pub const STATE_ACTIVE: f32 = 3.0;

/// Owns the texture and the revision it was built for.
pub struct ElementLut {
    texture: Option<Texture>,
    width: usize,
    height: usize,
    /// `(scene_generation, scene_state.revision)` of the data on the GPU.
    built: Option<(u64, u64)>,
}

impl Default for ElementLut {
    fn default() -> Self {
        ElementLut {
            texture: None,
            width: 0,
            height: 0,
            built: None,
        }
    }
}

impl ElementLut {
    pub fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()
    }

    /// `(width, height)` for the `elem_ctl` uniform.
    pub fn size(&self) -> (f32, f32) {
        (self.width as f32, self.height as f32)
    }

    /// Rebuild iff the scene or its state moved. Returns true when the GPU
    /// copy changed.
    pub fn sync(&mut self, cx: &mut Cx, state: &AppState) -> bool {
        let key = (state.scene.generation, state.scene_state.revision);
        if self.built == Some(key) && self.texture.is_some() {
            return false;
        }
        let n = state.scene.elements.len().max(1);
        let width = LUT_WIDTH;
        let height = (n + width - 1) / width;
        let realloc = self.texture.is_none() || self.width != width || self.height != height;
        let mut data = if realloc {
            vec![0.0f32; width * height * 4]
        } else {
            let t = self.texture.as_ref().unwrap();
            let mut d = t.take_vec_f32(cx);
            if d.len() != width * height * 4 {
                d = vec![0.0f32; width * height * 4];
            }
            d
        };
        fill(&mut data, state);
        if realloc {
            self.texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width,
                    height,
                    data: Some(data),
                    updated: TextureUpdated::Full,
                },
            ));
        } else {
            self.texture
                .as_ref()
                .unwrap()
                .put_back_vec_f32(cx, data, None);
        }
        self.width = width;
        self.height = height;
        self.built = Some(key);
        true
    }
}

fn fill(data: &mut [f32], state: &AppState) {
    let scene = &state.scene;
    let ss = &state.scene_state;
    let n = scene.elements.len();
    for i in 0..n {
        let id = ElementId::from_index(i);
        let o = i * 4;
        if o + 3 >= data.len() {
            break;
        }
        let visible = ss.is_visible(scene, id);
        let st = if !visible {
            STATE_HIDDEN
        } else if ss.selection.active == Some(id) {
            STATE_ACTIVE
        } else if ss.selection.contains(id) {
            STATE_SELECTED
        } else {
            STATE_VISIBLE
        };
        let off = to_render(explode_offset(scene, &ss.explode, id));
        data[o] = st;
        data[o + 1] = off.x;
        data[o + 2] = off.y;
        data[o + 3] = off.z;
    }
    // Every texel past the element count reads as hidden, so a stray index
    // never lights up a phantom.
    for i in n..(data.len() / 4) {
        data[i * 4] = STATE_HIDDEN;
        data[i * 4 + 1] = 0.0;
        data[i * 4 + 2] = 0.0;
        data[i * 4 + 3] = 0.0;
    }
}

/// Where an element sits in an exploded view, Fab space (Z up, meters).
///
/// `ExplodeState` documents the scale ("1 = one story height / bounds
/// radius") and nothing else, so this is the one implementation until lane A
/// ships the shared `ExplodeState::offset(&Scene, ElementId)` the review
/// (B2) ordered. **Delete this the day that lands** — the GPU lookup and
/// every CPU query (picking under explode, measuring) must not each carry
/// their own formula.
pub fn explode_offset(scene: &Scene, explode: &ExplodeState, id: ElementId) -> Vec3f {
    if explode.amount.abs() < 1e-4 {
        return vec3(0.0, 0.0, 0.0);
    }
    match explode.mode {
        ExplodeMode::ByStory => {
            let Some(e) = scene.element(id) else {
                return vec3(0.0, 0.0, 0.0);
            };
            let Some(story) = e.story else {
                return vec3(0.0, 0.0, 0.0);
            };
            let idx = story.index() as f32;
            let h = scene
                .stories
                .get(story.index())
                .map(|s| s.height)
                .filter(|h| *h > 0.01)
                .unwrap_or_else(|| {
                    let b = scene.bounds;
                    ((b.max.z - b.min.z) / scene.stories.len().max(1) as f32).max(2.5)
                });
            vec3(0.0, 0.0, explode.amount * h * idx)
        }
        ExplodeMode::ByElement => {
            let Some(e) = scene.element(id) else {
                return vec3(0.0, 0.0, 0.0);
            };
            if aabb_is_empty(&e.bounds) {
                return vec3(0.0, 0.0, 0.0);
            }
            let c = aabb_center(&scene.bounds);
            let d = aabb_center(&e.bounds) - c;
            let len = d.length();
            if len < 1e-4 {
                return vec3(0.0, 0.0, 0.0);
            }
            d * (explode.amount * aabb_radius(&scene.bounds) / len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Scene {
        Scene::from_model(crate::model::demo::demo_house(), &mut |_| {})
    }

    #[test]
    fn assembled_is_the_identity() {
        let scene = demo();
        let e = ExplodeState::default();
        for i in 0..scene.elements.len() {
            let o = explode_offset(&scene, &e, ElementId::from_index(i));
            assert_eq!(o, vec3(0.0, 0.0, 0.0));
        }
    }

    #[test]
    fn stories_slide_up_and_the_ground_floor_stays() {
        let scene = demo();
        let e = ExplodeState {
            amount: 1.0,
            mode: ExplodeMode::ByStory,
        };
        let mut moved = 0;
        for i in 0..scene.elements.len() {
            let id = ElementId::from_index(i);
            let o = explode_offset(&scene, &e, id);
            assert_eq!(o.x, 0.0, "by-story explode is Z only");
            assert_eq!(o.y, 0.0, "by-story explode is Z only");
            assert!(o.z >= 0.0);
            if o.z > 0.0 {
                moved += 1;
            }
            // The ground floor (story 0) never moves.
            if let Some(el) = scene.element(id) {
                if el.story.map(|s| s.index()) == Some(0) {
                    assert_eq!(o.z, 0.0);
                }
            }
        }
        assert!(moved > 0, "a two-story house must have something to lift");
    }

    #[test]
    fn lut_marks_hidden_and_selected() {
        let scene = demo();
        let mut state = AppState::default();
        state.scene = std::sync::Arc::new(scene);
        let first = ElementId::from_index(0);
        state.scene_state.set_hidden(first, true);
        let n = state.scene.elements.len();
        let mut data = vec![0.0f32; LUT_WIDTH * ((n + LUT_WIDTH - 1) / LUT_WIDTH) * 4];
        fill(&mut data, &state);
        assert_eq!(data[0], STATE_HIDDEN);
        state.scene_state.set_hidden(first, false);
        state.scene_state.select_only(first);
        fill(&mut data, &state);
        assert_eq!(data[0], STATE_ACTIVE);
    }
}
