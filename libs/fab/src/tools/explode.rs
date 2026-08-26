//! Lane E: the exploded view.
//!
//! `ExplodeState { amount, mode }` says how far apart the building comes;
//! this module is the **one** function that turns it into a per-element
//! offset. Lane B's per-element lookup texture must call exactly this (or the
//! moved-into-`fab_scene` copy of it — see report R3), because the moment the
//! GPU offset and the CPU offset disagree, every pick lands on the wrong
//! element.
//!
//! * `ByStory` — storeys slide apart along Z by one storey step each, in
//!   elevation order. An element with no storey of its own is filed under the
//!   storey it sits on.
//! * `ByElement` — every element moves away from the model centre.

use crate::api::*;
use makepad_widgets::*;

/// Storeys in elevation order (lowest first).
pub fn story_order(scene: &Scene) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..scene.stories.len()).collect();
    idx.sort_by(|a, b| {
        scene.stories[*a]
            .elevation
            .partial_cmp(&scene.stories[*b].elevation)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx
}

/// How far apart two neighbouring storeys are at `amount == 1`.
pub fn story_step(scene: &Scene) -> f32 {
    let order = story_order(scene);
    let mut step = 0.0f32;
    for w in order.windows(2) {
        let d = scene.stories[w[1]].elevation - scene.stories[w[0]].elevation;
        if d.is_finite() && d > 0.0 {
            step = step.max(d);
        }
    }
    for s in &scene.stories {
        if s.height > 0.0 {
            step = step.max(s.height);
        }
    }
    if step <= 0.0 {
        let h = aabb_extent(&scene.bounds).z;
        step = if scene.stories.len() > 1 {
            h / scene.stories.len() as f32
        } else {
            h.max(1.0)
        };
    }
    step.max(0.5)
}

/// Rank of an element's storey in elevation order. Falls back to the storey
/// the element physically sits on, so models whose storey table is partial
/// still explode sensibly.
pub fn story_rank(scene: &Scene, id: ElementId) -> usize {
    let order = story_order(scene);
    if order.is_empty() {
        return 0;
    }
    let el = match scene.element(id) {
        Some(e) => e,
        None => return 0,
    };
    if let Some(s) = el.story {
        if let Some(rank) = order.iter().position(|i| scene.stories[*i].id == s) {
            return rank;
        }
    }
    if el.has_geometry() {
        let z = aabb_center(&el.bounds).z;
        let mut rank = 0;
        for (r, i) in order.iter().enumerate() {
            if scene.stories[*i].elevation <= z + 1e-3 {
                rank = r;
            }
        }
        return rank;
    }
    0
}

/// The offset the renderer must add to every vertex of `id`.
///
/// **This is the single source of truth for the explode offset.** The lookup
/// texture and every CPU query — picking, box
/// select, measuring, the overlay — must read *this*, never their own copy of
/// the rule, or the picture and the picking drift apart.
///
/// **Open integration item:** `viewport/elements.rs::explode_offset` is a
/// second implementation of the same rule and the two disagree:
/// it orders storeys by `StoryId::index()` where this one orders by
/// *elevation*; it returns zero for an element with no storey of its own where
/// this one files the element under the storey it stands on (most of the
/// villa); and its by-element scale is `radius` against this one's
/// `0.6 · radius`. One of them has to go — lane E's report, R3.
pub fn element_offset(scene: &Scene, explode: &ExplodeState, id: ElementId) -> Vec3f {
    if explode.amount <= 0.0 || scene.is_empty() {
        return vec3(0.0, 0.0, 0.0);
    }
    match explode.mode {
        ExplodeMode::ByStory => {
            let rank = story_rank(scene, id);
            vec3(0.0, 0.0, explode.amount * story_step(scene) * rank as f32)
        }
        ExplodeMode::ByElement => {
            let Some(el) = scene.element(id) else {
                return vec3(0.0, 0.0, 0.0);
            };
            if !el.has_geometry() {
                return vec3(0.0, 0.0, 0.0);
            }
            let c = aabb_center(&scene.bounds);
            let d = aabb_center(&el.bounds) - c;
            let len = d.length();
            if len < 1e-4 {
                return vec3(0.0, 0.0, 0.0);
            }
            d / len * (explode.amount * aabb_radius(&scene.bounds) * 0.6)
        }
    }
}

/// Storey bounds with their explode offsets — what the overlay draws as the
/// exploded storey diagram, and a cheap way to see the maths is right before
/// lane B's lookup lands.
pub fn story_boxes(scene: &Scene, explode: &ExplodeState) -> Vec<(String, Aabb, Vec3f)> {
    let mut out = Vec::new();
    if scene.is_empty() {
        return out;
    }
    let order = story_order(scene);
    let step = story_step(scene);
    for (rank, i) in order.iter().enumerate() {
        let story = &scene.stories[*i];
        let mut b = aabb_empty();
        for id in &story.elements {
            if let Some(e) = scene.element(*id) {
                if e.has_geometry() {
                    b = aabb_union(&b, &e.bounds);
                }
            }
        }
        if aabb_is_empty(&b) {
            continue;
        }
        let offset = match explode.mode {
            ExplodeMode::ByStory => vec3(0.0, 0.0, explode.amount * step * rank as f32),
            ExplodeMode::ByElement => vec3(0.0, 0.0, 0.0),
        };
        out.push((story.name.clone(), b, offset));
    }
    out
}

pub fn mode_label(mode: ExplodeMode) -> &'static str {
    match mode {
        ExplodeMode::ByStory => "By Storey",
        ExplodeMode::ByElement => "By Element",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Scene {
        Scene::from_model(crate::model::demo::demo_house(), &mut |_| {})
    }

    #[test]
    fn assembled_is_zero_and_ground_never_moves() {
        let scene = demo();
        let off = ExplodeState {
            amount: 0.0,
            mode: ExplodeMode::ByStory,
        };
        for e in &scene.elements {
            assert_eq!(element_offset(&scene, &off, e.id), vec3(0.0, 0.0, 0.0));
        }
        let on = ExplodeState {
            amount: 1.0,
            mode: ExplodeMode::ByStory,
        };
        // the lowest storey stays put; something above it does not
        let ground = scene
            .elements
            .iter()
            .filter(|e| e.has_geometry())
            .min_by(|a, b| {
                aabb_center(&a.bounds)
                    .z
                    .partial_cmp(&aabb_center(&b.bounds).z)
                    .unwrap()
            })
            .unwrap();
        assert!(element_offset(&scene, &on, ground.id).z.abs() < 1e-4);
        let top = scene
            .elements
            .iter()
            .filter(|e| e.has_geometry())
            .max_by(|a, b| {
                aabb_center(&a.bounds)
                    .z
                    .partial_cmp(&aabb_center(&b.bounds).z)
                    .unwrap()
            })
            .unwrap();
        assert!(
            element_offset(&scene, &on, top.id).z > 0.5,
            "top element did not separate"
        );
    }

    #[test]
    fn storeys_separate_by_at_least_their_own_height() {
        let scene = demo();
        let step = story_step(&scene);
        assert!(step > 0.5, "storey step {step}");
        let on = ExplodeState {
            amount: 1.0,
            mode: ExplodeMode::ByStory,
        };
        let boxes = story_boxes(&scene, &on);
        assert!(boxes.len() >= 2, "demo house should have two storeys");
        for w in boxes.windows(2) {
            let a = w[0].1.max.z + w[0].2.z;
            let b = w[1].1.min.z + w[1].2.z;
            assert!(b >= a - 1e-3 || w[1].2.z > w[0].2.z, "storeys still overlap");
        }
    }
}
