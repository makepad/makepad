//! Structured identities.
//!
//! Definition-relative identities are what packages, presets, bindings and
//! saved editor navigation serialise, so a package stays reusable and no
//! runtime id ever enters a file. Runtime-qualified identities add the flow
//! instance and are never serialised into a package. None of them is a
//! dotted Splash name: every part is a separate field.

/// A package's stable identity (its manifest `id`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(pub String);

/// A module's path inside its package, `/`-separated, never absolute.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulePath(pub String);

/// A definition revision pinned by content: the lower-case hex sha256 of the
/// module source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefRevision(pub String);

/// One definition: a module of a package at a pinned revision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DefId {
    pub package: PackageId,
    pub module: ModulePath,
    pub revision: DefRevision,
}

/// A node's id inside its definition: the Splash field name in `Flow{...}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub String);

/// The chain of component instances leading to a node; empty at the root.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstancePath(pub Vec<NodeId>);

impl InstancePath {
    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DNode {
    pub path: InstancePath,
    pub node: NodeId,
}

impl DNode {
    /// A node of the root definition.
    pub fn root(node: impl Into<String>) -> Self {
        Self { path: InstancePath::root(), node: NodeId(node.into()) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DPort {
    pub node: DNode,
    pub port: String,
}

/// One entry of an ordered multi-connection input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DEdge {
    pub from: DPort,
    pub to: DPort,
    pub conn: Option<ConnectionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamRef {
    pub node: DNode,
    pub param: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BindingId(pub u64);

/// A definition's evaluation counter inside one engine (`Graph::revision`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

/// One live flow instance inside one engine. Local to that engine: it is
/// never serialised into a package and never sent to a session peer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub String);

/// The architecture's name for [`InstanceId`].
pub type FlowInstanceId = InstanceId;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QNode {
    pub instance: FlowInstanceId,
    pub def: DNode,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QPort {
    pub instance: FlowInstanceId,
    pub def: DPort,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct QEdge {
    pub instance: FlowInstanceId,
    pub def: DEdge,
}

/// A parameter of one live instance: what a control write addresses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamAddr {
    pub instance: FlowInstanceId,
    pub def: ParamRef,
}

/// One activation or prepared plan; a result stamped with an older
/// generation is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);
