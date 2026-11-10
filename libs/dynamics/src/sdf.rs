//! Replaces the C's SDF plugin callbacks.

pub trait Sdf: Send + Sync {
    fn distance(&self, point: &[f64; 3]) -> f64;
    fn gradient(&self, gradient: &mut [f64; 3], point: &[f64; 3]);
    /// The distance and the gradient at one point, as [`distance`](Self::distance) and
    /// [`gradient`](Self::gradient) give them: a field that finds both at once (a sampled grid) answers
    /// with one lookup.
    fn distance_gradient(&self, gradient: &mut [f64; 3], point: &[f64; 3]) -> f64 {
        self.gradient(gradient, point);
        self.distance(point)
    }
}

/// Make geom `geom` of a compiled model the field `sdf`, a plugin SDF: the builder takes no plugins, so
/// a field geom is built as a stand-in (a box over the field's bounds gives the broad phase its extent)
/// and attached after compile, before the model's data is made. The plugin has no state, attributes or
/// name.
pub fn attach(m: &mut crate::model::Model, geom: usize, sdf: std::sync::Arc<dyn Sdf>) {
    let instance = m.sdf_plugins.len();
    m.geom_type[geom] = crate::types::GEOM_SDF;
    m.geom_plugin[geom] = instance as i32;
    m.sdf_plugins.push(SdfPlugin(sdf));
    m.nplugin = m.sdf_plugins.len();
    m.plugin.push(-1);
    m.plugin_stateadr.push(0);
    m.plugin_statenum.push(0);
    m.plugin_attradr.push(-1);
    // an unnamed object's name is the empty string (the names' last terminator); the plugins' part of
    // the name map comes last, two slots each, empty
    m.name_pluginadr.push(m.nnames as i32 - 1);
    m.names_map.extend([-1, -1]);
    m.nnames_map += 2;
}

#[derive(Clone)]
pub struct SdfPlugin(pub std::sync::Arc<dyn Sdf>);

impl std::fmt::Debug for SdfPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SdfPlugin")
    }
}

impl PartialEq for SdfPlugin {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}
