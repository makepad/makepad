//! Cross-arena geometry adapter for offset vertex caps.
//!
//! Upstream: `source/SMLib/src/SmOffsetExecutive.cpp` (3227–3450),
//! `source/SMLib/src/SmOffsetGeometryCreation.cpp` (`VertexOffset`).
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `build` (private) | `BuildOffsetOfVertex` calls to `VertexOffset` |
//! | `VertexBrep` (private) | Source vertex, result edges and newly owned geometry |
//!
//! Deviations: tagged typed ids distinguish source vertices from result topology;
//! owned Rust values replace the creator's output pointers. The shared creator
//! algorithm and concrete curve kinds are unchanged. No new recursion.

use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::kernel::Kernel;
use crate::math::Vec3;
use crate::offset::offset_geometry_creation::{
    OffsetGeometryCreation, VertexOffsetBrep, VertexOffsetGeometry,
};
use crate::surface::surface::Surface;
use crate::topology::{edge, vertex, Brep, EdgeId, FaceId, VertexId};

use super::p1::VertexOffset;

const OP: &str = "offset::offset_executive::vertex";

#[derive(Clone, Copy)]
enum Handle {
    OriginalVertex(VertexId),
    OffsetEdge(EdgeId),
    OffsetFace(FaceId),
    OffsetCurve(EdgeId),
    NewCurve(usize),
    NewSurface,
}

fn invalid_handle() -> SolidError {
    SolidError::invalid_input(
        OP,
        "vertex creator handle does not resolve to the requested kind",
    )
}

struct VertexBrep<'a> {
    original: &'a Brep,
    offset: &'a mut Brep,
    curves: Vec<Option<Curve>>,
    surface: Option<Surface>,
}

impl VertexBrep<'_> {
    fn take_output(mut self, made: VertexOffsetGeometry<Handle>) -> Result<VertexOffset> {
        let surface = match made.surface {
            Some(Handle::NewSurface) => Some(self.surface.take().ok_or_else(invalid_handle)?),
            None => None,
            Some(_) => return Err(invalid_handle()),
        };
        let mut trim_curves = Vec::new();
        for handle in made.trimming_curves {
            let Handle::NewCurve(index) = handle else {
                return Err(invalid_handle());
            };
            trim_curves.push(
                self.curves
                    .get_mut(index)
                    .and_then(Option::take)
                    .ok_or_else(invalid_handle)?,
            );
        }
        Ok(VertexOffset {
            surface,
            trim_curves,
            curve_orientations: made.orients,
        })
    }
}

impl VertexOffsetBrep<Handle> for VertexBrep<'_> {
    fn brep_tolerance(&self, vertex: Handle) -> Result<f64> {
        let Handle::OriginalVertex(vertex) = vertex else {
            return Err(invalid_handle());
        };
        self.original.vertex(vertex)?;
        Ok(self.original.tolerance())
    }

    fn vertex_point(&self, vertex: Handle) -> Result<Vec3> {
        let Handle::OriginalVertex(vertex) = vertex else {
            return Err(invalid_handle());
        };
        vertex::point(self.original, vertex)
    }

    fn vertex_is_lamina(&self, vertex: Handle) -> Result<bool> {
        let Handle::OriginalVertex(vertex) = vertex else {
            return Err(invalid_handle());
        };
        vertex::is_lamina_vertex(self.original, vertex)
    }

    fn edge_faces(&self, e: Handle) -> Result<Vec<Handle>> {
        let Handle::OffsetEdge(e) = e else {
            return Err(invalid_handle());
        };
        Ok(edge::faces(self.offset, e, None)?
            .into_iter()
            .map(Handle::OffsetFace)
            .collect())
    }

    fn edge_is_seam_of_face_surface(&mut self, k: &Kernel, e: Handle, f: Handle) -> Result<bool> {
        let (Handle::OffsetEdge(e), Handle::OffsetFace(f)) = (e, f) else {
            return Err(invalid_handle());
        };
        edge::is_seam_of_surface(k, self.offset, e, f)
    }

    fn edge_curve(&self, e: Handle) -> Result<Option<Handle>> {
        let Handle::OffsetEdge(e) = e else {
            return Err(invalid_handle());
        };
        Ok(self
            .offset
            .edge(e)?
            .curve
            .as_ref()
            .map(|_| Handle::OffsetCurve(e)))
    }

    fn edge_interval(&self, e: Handle) -> Result<Extent1d> {
        let Handle::OffsetEdge(e) = e else {
            return Err(invalid_handle());
        };
        edge::interval(self.offset, e)
    }

    fn curve(&self, curve: Handle) -> Result<&Curve> {
        let Handle::OffsetCurve(edge) = curve else {
            return Err(invalid_handle());
        };
        self.offset
            .edge(edge)?
            .curve
            .as_ref()
            .ok_or_else(invalid_handle)
    }

    fn insert_curve(&mut self, curve: Curve) -> Result<Handle> {
        let handle = Handle::NewCurve(self.curves.len());
        self.curves.push(Some(curve));
        Ok(handle)
    }

    fn insert_surface(&mut self, surface: Surface) -> Result<Handle> {
        self.surface = Some(surface);
        Ok(Handle::NewSurface)
    }
}

pub(super) fn build(
    kernel: &Kernel,
    creator: &OffsetGeometryCreation,
    original: &Brep,
    vertex: VertexId,
    offset: &mut Brep,
    bounding_edges: &[EdgeId],
) -> Result<VertexOffset> {
    let mut backend = VertexBrep {
        original,
        offset,
        curves: Vec::new(),
        surface: None,
    };
    let edges: Vec<_> = bounding_edges
        .iter()
        .copied()
        .map(Handle::OffsetEdge)
        .collect();
    let made = creator.vertex_offset_with_backend(
        kernel,
        &mut backend,
        Some(Handle::OriginalVertex(vertex)),
        &edges,
    )?;
    backend.take_output(made)
}
