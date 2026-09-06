use crate::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationKind {
    BoundaryEdge,
    LooseEdge,
    IsolatedVertex,
    NonManifoldEdge,
    NonManifoldVertex,
    InconsistentWinding,
    RepeatedVertex,
    DegenerateEdge,
    DegenerateFace,
    NonPlanarFace,
    SelfIntersectingFace,
    TriangulationFailed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub kind: ValidationKind,
    pub element: ElementId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfIntersectionStatus {
    NotChecked,
    Clear,
    Found,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationReport {
    pub is_surface_manifold: bool,
    pub is_closed: bool,
    pub is_consistently_oriented: bool,
    pub is_valid_surface: bool,
    /// Valid local geometry plus closed, consistently oriented manifold topology.
    /// This does not certify freedom from intersections between separate faces.
    pub is_closed_manifold: bool,
    /// Conservative embedded-solid qualification. Always false while global
    /// self-intersection status is `NotChecked`.
    pub is_valid_solid: bool,
    pub boundary_edges: usize,
    pub loose_edges: usize,
    pub isolated_vertices: usize,
    pub non_manifold_edges: usize,
    pub non_manifold_vertices: usize,
    pub issues: Vec<ValidationIssue>,
    pub self_intersections: SelfIntersectionStatus,
}

impl Mesh {
    pub fn validate(&self, ctx: &mut Context<'_>) -> Result<ValidationReport> {
        self.check_structure(ctx)?;
        let adjacency = self.adjacency(ctx)?;
        let mut report = ValidationReport {
            is_surface_manifold: true,
            is_closed: !self.faces.is_empty(),
            is_consistently_oriented: true,
            is_valid_surface: true,
            is_closed_manifold: false,
            is_valid_solid: false,
            boundary_edges: 0,
            loose_edges: 0,
            isolated_vertices: 0,
            non_manifold_edges: 0,
            non_manifold_vertices: 0,
            issues: Vec::new(),
            self_intersections: SelfIntersectionStatus::NotChecked,
        };
        let mut bad_geometry = false;
        for f in &self.faces {
            match crate::geometry::face_geometry(self, f.id, ctx) {
                Ok(g) => {
                    if !g.planar {
                        report.issues.push(ValidationIssue {
                            kind: ValidationKind::NonPlanarFace,
                            element: ElementId::Face(f.id),
                        });
                    }
                    match crate::geometry::ear_clip(&g.points, f.id, ctx) {
                        Ok(_) => {}
                        Err(MeshError::InvalidGeometry { kind, .. }) => {
                            bad_geometry = true;
                            report.issues.push(ValidationIssue {
                                kind,
                                element: ElementId::Face(f.id),
                            });
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(MeshError::InvalidGeometry { kind, .. }) => {
                    bad_geometry = true;
                    report.issues.push(ValidationIssue {
                        kind,
                        element: ElementId::Face(f.id),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        for (&edge, uses) in &adjacency.edges {
            ctx.checkpoint(1)?;
            let kind = match uses.len() {
                0 => {
                    report.loose_edges += 1;
                    Some(ValidationKind::LooseEdge)
                }
                1 => {
                    report.boundary_edges += 1;
                    Some(ValidationKind::BoundaryEdge)
                }
                2 => {
                    if uses[0].from == uses[1].from {
                        report.is_consistently_oriented = false;
                        Some(ValidationKind::InconsistentWinding)
                    } else {
                        None
                    }
                }
                _ => {
                    report.non_manifold_edges += 1;
                    Some(ValidationKind::NonManifoldEdge)
                }
            };
            if let Some(kind) = kind {
                report.issues.push(ValidationIssue {
                    kind,
                    element: ElementId::Edge(edge),
                });
            }
        }
        for (&v, faces) in &adjacency.vertex_faces {
            ctx.checkpoint(1)?;
            let edges = &adjacency.vertex_edges[&v];
            if faces.is_empty() {
                if edges.is_empty() {
                    report.isolated_vertices += 1;
                    report.issues.push(ValidationIssue {
                        kind: ValidationKind::IsolatedVertex,
                        element: ElementId::Vertex(v),
                    });
                }
                continue;
            }
            let mut neighbors: BTreeMap<FaceId, Vec<FaceId>> =
                faces.iter().map(|&f| (f, Vec::new())).collect();
            let mut boundary = 0;
            let mut nonmanifold = false;
            for edge in edges {
                ctx.checkpoint(1)?;
                let uses = &adjacency.edges[edge];
                match uses.len() {
                    0 => {}
                    1 => boundary += 1,
                    2 => {
                        neighbors.get_mut(&uses[0].face).unwrap().push(uses[1].face);
                        neighbors.get_mut(&uses[1].face).unwrap().push(uses[0].face);
                    }
                    _ => nonmanifold = true,
                }
            }
            let mut visited = BTreeSet::new();
            let mut stack = vec![faces[0]];
            while let Some(f) = stack.pop() {
                ctx.checkpoint(1)?;
                if visited.insert(f) {
                    stack.extend(neighbors[&f].iter().copied());
                }
            }
            if nonmanifold || (boundary != 0 && boundary != 2) || visited.len() != faces.len() {
                report.non_manifold_vertices += 1;
                report.issues.push(ValidationIssue {
                    kind: ValidationKind::NonManifoldVertex,
                    element: ElementId::Vertex(v),
                });
            }
        }
        report.is_surface_manifold =
            report.non_manifold_edges == 0 && report.non_manifold_vertices == 0;
        report.is_closed &=
            report.boundary_edges == 0 && report.loose_edges == 0 && report.isolated_vertices == 0;
        report.is_valid_surface =
            !bad_geometry && report.is_surface_manifold && report.is_consistently_oriented;
        report.is_closed_manifold = report.is_valid_surface && report.is_closed;
        report.is_valid_solid = false;
        Ok(report)
    }
}
