//! A producer-owned presentation snapshot. Mutable access requires explicit revision updates.
use crate::*;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct World {
    pub entities: Vec<Entity>,
    pub parts: Vec<Part>,
    pub beams: Vec<Beam>,
    pub labels: Vec<LabelDef>,
    pub decals: Vec<Decal>,
    pub terrain: Option<Arc<Terrain>>,
    pub terrain_materials: Option<Arc<TerrainMaterials>>,
    pub voxel: Option<Arc<VoxelView>>,
    pub water: Option<Arc<WaterView>>,
    pub water_time: f32,
    pub sky: Option<SkyConfig>,
    pub sun: SunConfig,
    pub camera: CameraState,
    pub hud: HudDoc,
    pub hud_slots: Vec<(String, HudSlot)>,
    pub hud_bars: Vec<HudBar>,
    pub crosshair: bool,
    pub render_rev: u64,
    pub paint_rev: u64,
    pub content_generation: u64,
}
impl World {
    pub fn new() -> Self { Self::default() }
    pub fn entity(&self, id: u64) -> Option<&Entity> {
        entity_index_sorted(&self.entities, id).map(|i| &self.entities[i])
    }
    /// Mark structural or paint changes explicitly after editing an entity.
    pub fn entity_mut(&mut self, id: u64) -> Option<&mut Entity> {
        entity_index_sorted(&self.entities, id).map(|i| &mut self.entities[i])
    }
    /// Insert an externally identified entity in sorted order; duplicates fail atomically.
    pub fn push_entity(&mut self, entity: Entity) -> Result<(), String> {
        match self.entities.binary_search_by_key(&entity.id, |e| e.id) {
            Ok(_) => Err(format!("duplicate scene entity id {}", entity.id)),
            Err(index) => {
                self.entities.insert(index, entity);
                self.mark_render_dirty();
                Ok(())
            }
        }
    }
    pub fn mark_render_dirty(&mut self) { self.render_rev = self.render_rev.wrapping_add(1); }
    pub fn mark_paint_dirty(&mut self) { self.paint_rev = self.paint_rev.wrapping_add(1); }
}
pub fn entity_index_sorted(entities: &[Entity], id: u64) -> Option<usize> {
    entities.binary_search_by_key(&id, |e| e.id).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ent(id: u64) -> Entity { Entity { id, ..Default::default() } }
    #[test]
    fn a_repaint_moves_paint_rev_and_leaves_the_geometry_revision_alone() {
        let mut w = World::new();
        let (render, paint) = (w.render_rev, w.paint_rev);
        w.mark_paint_dirty();
        assert_eq!(w.render_rev, render);
        assert_eq!(w.paint_rev, paint.wrapping_add(1));
        w.mark_render_dirty();
        assert_eq!(w.render_rev, render.wrapping_add(1));
        assert_eq!(w.paint_rev, paint.wrapping_add(1));
    }

    #[test]
    fn lookup_after_push_and_retain() {
        let mut w = World::default();
        for id in [1u64, 2, 5, 9, 12] {
            w.push_entity(ent(id)).unwrap();
        }
        assert!(w.entities.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert_eq!(w.entity(5).map(|e| e.id), Some(5));
        assert!(w.entity(3).is_none());
        w.entities.retain(|e| e.id != 5);
        assert!(w.entities.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert!(w.entity(5).is_none());
        assert_eq!(w.entity_mut(12).map(|e| e.id), Some(12));
        assert_eq!(entity_index_sorted(&w.entities, 9), Some(2));
    }

}
