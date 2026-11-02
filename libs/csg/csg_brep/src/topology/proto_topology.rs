//! Proto topology: the intersection results of a Boolean gathered into proto vertices,
//! edges and faces before they are resolved and imprinted (`SmProtoTopology`), plus the
//! owning-topology lists, assemblies, copy maps and API transforms that share its plan
//! unit.
//!
//! Upstream: `source/SMLib/src/SmProtoTopology.cpp`, `source/SMLib/inc/SmProtoTopology.h`.
//! The file is split over several plan units (`topology.proto_topology_p1`, `_p2`,
//! `_p3`); each lives in its own submodule and adds methods to the types of
//! [`p1`].

pub mod p1;
pub mod p2;
pub mod p3;

pub use p1::{
    EdgeDefId, EdgeDefinition, OwningRecords, OwningTopology, ProtoEdge, ProtoEdgeId, ProtoFace,
    ProtoFaceId, ProtoGeometry, ProtoTopologyManager, ProtoVertex, ProtoVertexId, VertexDefId,
    VertexDefinition,
};
pub use p2::ProtoBrep;
