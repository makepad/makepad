//! Host-provided perception and routing for geometry outside the sim.
//!
//! A streamed level is not represented by ordinary sim entities: its walls
//! are triangles and its route graph lives with the renderer's level data.
//! The host installs these two read-only providers beside `GameWorld::level`.
//! With no provider installed, callers retain their pre-level behaviour.

use makepad_math::Vec3f;

use crate::GameWorld;

/// Answers whether level geometry blocks the segment from `a` to `b`.
///
/// `Send + Sync` lets cloned world snapshots share one immutable provider.
pub trait LosProvider: Send + Sync {
    fn blocked(&self, a: Vec3f, b: Vec3f) -> bool;
}

/// Answers one level-navigation query.
///
/// The returned point is the next waypoint toward `to`. `None` means the
/// provider has no path; the caller may fall back to direct steering.
pub trait NavProvider: Send + Sync {
    fn next_step(&self, from: Vec3f, to: Vec3f, radius: f32) -> Option<Vec3f>;
}

impl GameWorld {
    /// Level LOS with the provider-absent fallback made explicit.
    #[inline]
    pub fn los_blocked(&self, a: Vec3f, b: Vec3f) -> bool {
        self.los.as_ref().is_some_and(|los| los.blocked(a, b))
    }

    /// Level navigation with the provider-absent/no-path fallback made
    /// explicit. Callers decide how to steer when this returns `None`.
    #[inline]
    pub fn level_nav_next_step(
        &self,
        from: Vec3f,
        to: Vec3f,
        radius: f32,
    ) -> Option<Vec3f> {
        self.nav_provider
            .as_ref()
            .and_then(|nav| nav.next_step(from, to, radius))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_math::vec3f;
    use std::sync::Arc;

    struct Wall;

    impl LosProvider for Wall {
        fn blocked(&self, a: Vec3f, b: Vec3f) -> bool {
            a.x < 1.0 && b.x > 1.0
        }
    }

    struct Corner;

    impl NavProvider for Corner {
        fn next_step(&self, from: Vec3f, to: Vec3f, radius: f32) -> Option<Vec3f> {
            (radius >= 0.5).then_some(vec3f(from.x, to.y, to.z))
        }
    }

    #[test]
    fn provider_absence_is_clear_los_and_no_level_path() {
        let world = GameWorld::new();
        let a = vec3f(0.0, 0.0, 0.0);
        let b = vec3f(2.0, 0.0, 3.0);
        assert!(!world.los_blocked(a, b));
        assert_eq!(world.level_nav_next_step(a, b, 0.5), None);
    }

    #[test]
    fn installed_providers_supply_their_answers() {
        let mut world = GameWorld::new();
        world.los = Some(Arc::new(Wall));
        world.nav_provider = Some(Arc::new(Corner));
        let a = vec3f(0.0, 0.0, 0.0);
        let b = vec3f(2.0, 0.0, 3.0);
        assert!(world.los_blocked(a, b));
        assert_eq!(
            world.level_nav_next_step(a, b, 0.5),
            Some(vec3f(0.0, 0.0, 3.0))
        );
        assert_eq!(world.level_nav_next_step(a, b, 0.25), None);
    }
}
