//! Affine physical skin overrides. A cell may shear; attached face frames
//! are rigid. Ordinary animation below either frame keeps its local pose.
use super::*;

impl SkinnedModel {
    /// Overrides are palette ordinals and rest-mesh -> posed-mesh matrices.
    /// Descendant animation is evaluated below the overridden joint, so gaze,
    /// blink and limb clips remain independent of the physical attachment.
    pub fn palette_with_affine_overrides(&self, pose: &PoseBuffer,
        overrides: &[(u16, Mat4f)], out: &mut Vec<Mat4f>) {
        if overrides.is_empty() { self.palette(pose, out); return; }
        let mut forced = vec![None; self.nodes.len()];
        for &(joint, matrix) in overrides {
            let j = joint as usize;
            if let (Some(&node), Some(bind)) = (self.joint_nodes.get(j), self.inverse_bind.get(j)) {
                if matrix.v.iter().all(|v|v.is_finite()) {
                    forced[node] = Some(Mat4f::mul(&matrix, &bind.invert()));
                }
            }
        }
        let rest_mesh_inv = self.node_global(pose, self.mesh_node).invert();
        let mut globals = vec![None; self.nodes.len()];
        fn visit(model: &SkinnedModel, pose: &PoseBuffer, mesh_inv: &Mat4f,
            forced: &[Option<Mat4f>], globals: &mut [Option<Mat4f>], i: usize) -> Mat4f {
            if let Some(matrix) = globals[i] { return matrix; }
            let matrix = if let Some(matrix) = forced[i] { matrix } else {
                let local = trs_to_mat4(pose.get(i).unwrap_or(&model.nodes[i].rest));
                let parent = model.nodes[i].parent.map(|parent|
                    visit(model, pose, mesh_inv, forced, globals, parent)).unwrap_or(*mesh_inv);
                Mat4f::mul(&parent, &local)
            };
            globals[i] = Some(matrix); matrix
        }
        out.clear();
        for (j, &node) in self.joint_nodes.iter().enumerate() {
            let global = visit(self, pose, &rest_mesh_inv, &forced, &mut globals, node);
            out.push(Mat4f::mul(&global, &self.inverse_bind[j]));
        }
    }

    fn node_global(&self, pose: &PoseBuffer, node: usize) -> Mat4f {
        let local = trs_to_mat4(pose.get(node).unwrap_or(&self.nodes[node].rest));
        self.nodes[node].parent.map(|parent|Mat4f::mul(&self.node_global(pose, parent), &local)).unwrap_or(local)
    }

    /// Sockets/lights use the same physical frame as the rendered skin.
    pub fn node_mesh_transform_from_palette(&self, pose: &PoseBuffer,
        palette: &[Mat4f], node: usize) -> Option<Mat4f> {
        let source = self.nodes.get(node)?;
        if let Some(j) = self.joint_nodes.iter().position(|&joint|joint == node) {
            return palette.get(j).map(|matrix|Mat4f::mul(matrix, &self.inverse_bind[j].invert()));
        }
        if let Some(parent) = source.parent {
            let parent = self.node_mesh_transform_from_palette(pose, palette, parent)?;
            return Some(Mat4f::mul(&parent, &trs_to_mat4(pose.get(node).unwrap_or(&source.rest))));
        }
        self.node_mesh_transform(pose, node)
    }
}

/// Inverse transpose of the position matrix. The finite singular fallback
/// prevents invalid normals while a rejected physical frame is replaced.
pub(crate) fn affine_normal(m: &Mat4f, normal: Vec3f) -> Vec3f {
    let row = |i| Vec3f {x:m.v[i], y:m.v[i+4], z:m.v[i+8]};
    let (a,b,c) = (row(0),row(1),row(2));
    let bc = Vec3f::cross(b,c);
    let dot = |a:Vec3f,b:Vec3f|a.x*b.x+a.y*b.y+a.z*b.z;
    let det = dot(a,bc);
    if !det.is_finite() || det.abs()<1e-8 { return mat4_mul_dir(m, normal); }
    Vec3f{x:dot(bc,normal)/det,y:dot(Vec3f::cross(c,a),normal)/det,z:dot(Vec3f::cross(a,b),normal)/det}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn affine_normals_remain_perpendicular_under_shear_and_scale() {
        let mut matrix=Mat4f::identity();
        matrix.v[0]=1.7;matrix.v[4]=0.6;matrix.v[5]=0.4;matrix.v[9]=0.2;
        let n=affine_normal(&matrix,Vec3f{x:1.,y:0.,z:0.});
        for tangent in [Vec3f{x:0.,y:1.,z:0.},Vec3f{x:0.,y:0.,z:1.}] {
            let t=mat4_mul_dir(&matrix,tangent);
            assert!((n.x*t.x+n.y*t.y+n.z*t.z).abs()<1e-6);
        }
        assert!((n.x-1./1.7).abs()<1e-6);
    }
}
