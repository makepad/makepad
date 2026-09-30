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
    pub crosshair: bool,
    pub render_rev: u64,
    pub paint_rev: u64,
    pub content_generation: u64,
    // --- generic items (KERNELS.md §3.2). Empty for a host that fills only
    // the game features above; the renderer draws nothing new from them yet.
    pub view: View,
    pub lights: Vec<Light>,
    pub environment: Environment,
    pub materials: Vec<MaterialFrame>,
    pub items: Vec<Item>,
    pub anchors: Vec<Anchor>,
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
    /// This frame's material by id (`materials` is kept sorted by id).
    pub fn material(&self, id: MaterialId) -> Option<&MaterialFrame> {
        self.materials.binary_search_by_key(&id, |m| m.id).ok().map(|i| &self.materials[i])
    }
    /// Insert or replace a material, keeping `materials` sorted by id.
    pub fn set_material(&mut self, material: MaterialFrame) {
        match self.materials.binary_search_by_key(&material.id, |m| m.id) {
            Ok(i) => self.materials[i] = material,
            Err(i) => self.materials.insert(i, material),
        }
    }
    /// Check the generic half of the frame before the renderer sees it:
    /// every value finite and in range, every item's materials present,
    /// the light count bounded. The first problem is reported with where
    /// it is.
    pub fn validate_frame(&self) -> Result<(), String> {
        if let Some(camera) = &self.view.camera {
            camera.validate().map_err(|e| format!("view: {e}"))?;
        }
        self.environment.validate().map_err(|e| format!("environment: {e}"))?;
        if self.lights.len() > MAX_WORLD_LIGHTS {
            return Err(format!("{} lights; at most {MAX_WORLD_LIGHTS}", self.lights.len()));
        }
        for (i, light) in self.lights.iter().enumerate() {
            light.validate().map_err(|e| format!("lights[{i}]: {e}"))?;
        }
        if !self.materials.windows(2).all(|w| w[0].id < w[1].id) {
            return Err("materials must be sorted by id, without duplicates".into());
        }
        for m in &self.materials {
            m.validate().map_err(|e| format!("material {}: {e}", m.id.0))?;
        }
        for (i, item) in self.items.iter().enumerate() {
            item.validate().map_err(|e| format!("items[{i}]: {e}"))?;
            if let Some(missing) = item.materials().find(|id| self.material(*id).is_none()) {
                return Err(format!("items[{i}]: material {} is not in this frame", missing.0));
            }
        }
        for (i, a) in self.anchors.iter().enumerate() {
            if !(a.pos.x.is_finite() && a.pos.y.is_finite() && a.pos.z.is_finite()) {
                return Err(format!("anchors[{i}] '{}': position must be finite", a.name));
            }
        }
        Ok(())
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
    fn generic_items_validate_against_the_frame_materials() {
        let mut w = World::new();
        assert!(w.validate_frame().is_ok(), "an empty frame is valid");
        let item = Item::new(ItemKind::Mesh { geometry: GeometryRef::Resident(GeometryId(1)), material: MaterialId(4), transform: makepad_math::Mat4f::identity() });
        w.items.push(item);
        assert!(w.validate_frame().unwrap_err().contains("material 4"));
        w.set_material(MaterialFrame { id: MaterialId(9), ..Default::default() });
        w.set_material(MaterialFrame { id: MaterialId(4), ..Default::default() });
        w.set_material(MaterialFrame { id: MaterialId(4), glow: 2.0, ..Default::default() });
        assert_eq!(w.materials.iter().map(|m| m.id.0).collect::<Vec<_>>(), vec![4, 9]);
        assert_eq!(w.material(MaterialId(4)).unwrap().glow, 2.0);
        assert!(w.validate_frame().is_ok());
        w.view.camera = Some(Camera { near: -1.0, ..Camera::default() });
        assert!(w.validate_frame().unwrap_err().starts_with("view"));
        w.view.camera = Some(Camera::default());
        w.anchors.push(Anchor { name: "a".into(), pos: makepad_math::vec3f(f32::NAN, 0.0, 0.0) });
        assert!(w.validate_frame().is_err());
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
