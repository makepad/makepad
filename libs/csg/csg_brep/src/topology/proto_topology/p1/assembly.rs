//! `SmAssembly` and `SmAssemblyInstance`: assembly trees whose components are
//! instances (placed, scaled, optionally mirrored uses of a B-rep or a
//! sub-assembly) kept on the assembly's owning-topology list.

use crate::error::Result;
use crate::geom::a_object;
use crate::geom::attribute::{AttributeStore, ObjectKey};
use crate::geom::axis2_placement::Axis2Placement;
use crate::math::Vec3;

use super::owning_topology::{destroy_topology, OwningRecords};

const OP: &str = "topology::proto_topology::p1::assembly";

/// `SmAssembly_TYPE` (`TOPO_BASE_TYPE + 55`).
pub const ASSEMBLY_TYPE: i64 = 16055;

/// `SmAssembly::GetTypeString`.
pub const ASSEMBLY_TYPE_STRING: &str = "SmAssembly_TYPE";

/// `SmAssembly::GetClassString`.
pub const ASSEMBLY_CLASS_STRING: &str = "SmAssembly";

/// `SmAssemblyInstance_TYPE` (`TOPO_BASE_TYPE + 56`).
pub const ASSEMBLY_INSTANCE_TYPE: i64 = 16056;

/// `SmAssemblyInstance::GetTypeString`.
pub const ASSEMBLY_INSTANCE_TYPE_STRING: &str = "SmAssemblyInstance_TYPE";

/// `SmAssemblyInstance::GetClassString`.
pub const ASSEMBLY_INSTANCE_CLASS_STRING: &str = "SmAssemblyInstance";

/// `SmAssembly::IsKindOf`: [`ASSEMBLY_TYPE`], then the `SmSAGObject` chain (which
/// skips `SmOwningTopology` and `SmTopology`).
#[must_use]
pub const fn assembly_is_kind_of(t: i64) -> bool {
    if t == ASSEMBLY_TYPE {
        true
    } else {
        a_object::sag_is_kind_of(t)
    }
}

/// `SmAssemblyInstance::IsKindOf`: [`ASSEMBLY_INSTANCE_TYPE`], then the
/// `SmSAGObject` chain.
#[must_use]
pub const fn assembly_instance_is_kind_of(t: i64) -> bool {
    if t == ASSEMBLY_INSTANCE_TYPE {
        true
    } else {
        a_object::sag_is_kind_of(t)
    }
}

fn list_of<R: OwningRecords + ?Sized>(
    records: &R,
    this: ObjectKey,
) -> Result<super::owning_topology::OwningTopology> {
    records
        .owning(this)
        .ok_or(crate::error::SolidError::invalid_input(
            OP,
            "object has no list data",
        ))
}

fn store_list<R: OwningRecords + ?Sized>(
    records: &mut R,
    this: ObjectKey,
    list: super::owning_topology::OwningTopology,
) -> Result<()> {
    records
        .set_owning(this, list)
        .ok_or(crate::error::SolidError::invalid_input(
            OP,
            "object has no list data",
        ))
}

/// An assembly (`SmAssembly`): a name and a user pointer; its instances are the
/// members of its list (in the [`OwningRecords`] store, under the assembly's key).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Assembly {
    /// `m_pName` (`None` for a null name).
    name: Option<String>,
    /// `m_pUserPointer` (opaque; `0` is null).
    user_pointer: u64,
}

impl Assembly {
    /// `SmAssembly(pName)`.
    #[must_use]
    pub fn new(name: Option<&str>) -> Self {
        Self {
            name: name.map(str::to_owned),
            user_pointer: 0,
        }
    }

    /// The name (`GetName`).
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The user pointer (`GetUserPointer`; `0` is null).
    #[must_use]
    pub const fn user_pointer(&self) -> u64 {
        self.user_pointer
    }

    /// Sets the user pointer (`SetUserPointer`).
    pub fn set_user_pointer(&mut self, user_pointer: u64) {
        self.user_pointer = user_pointer;
    }

    /// The instances of the assembly `this`, in list order (`GetAssemblyInstances`:
    /// `GetAll` with its status ignored).
    ///
    /// # Errors
    ///
    /// [`crate::SolidError::InvalidInput`] for a broken list or an assembly without list
    /// data.
    pub fn assembly_instances<R: OwningRecords + ?Sized>(
        records: &R,
        this: ObjectKey,
    ) -> Result<Vec<ObjectKey>> {
        list_of(records, this)?.get_all_ignoring_status(records, &AttributeStore::new(), None, None)
    }

    /// Adds `instance` at the end of the assembly's list (`AddInstance`, private, used
    /// by the instance constructor).
    ///
    /// # Errors
    ///
    /// Those of [`super::OwningTopology::post_insert`].
    pub fn add_instance<R: OwningRecords + ?Sized>(
        records: &mut R,
        this: ObjectKey,
        instance: ObjectKey,
    ) -> Result<()> {
        let mut list = list_of(records, this)?;
        list.post_insert(this, records, instance)?;
        store_list(records, this, list)
    }

    /// Removes `instance` from the assembly's list (`RemoveInstance`; a failure is only
    /// reported upstream).
    ///
    /// # Errors
    ///
    /// [`crate::SolidError::InvalidInput`] for an object without a record or list data.
    pub fn remove_instance<R: OwningRecords + ?Sized>(
        records: &mut R,
        this: ObjectKey,
        instance: ObjectKey,
    ) -> Result<()> {
        let mut list = list_of(records, this)?;
        let status = list.remove(this, records, instance);
        store_list(records, this, list)?;
        match status {
            Err(
                e @ (crate::error::SolidError::InvalidInput { .. }
                | crate::error::SolidError::PortInvariant { .. }),
            ) => Err(e),
            Ok(()) | Err(_) => Ok(()),
        }
    }

    /// The destructor `~SmAssembly` of the assembly `this`: every instance is removed
    /// from its list (statuses ignored), the list is forgotten
    /// (`~SmOwningTopology`), and the assembly leaves its own owner's list
    /// (`~SmTopology`).
    ///
    /// # Errors
    ///
    /// [`crate::SolidError::InvalidInput`] for a broken list or missing records.
    pub fn destroy<R: OwningRecords + ?Sized>(
        self,
        records: &mut R,
        this: ObjectKey,
    ) -> Result<()> {
        let instances = Self::assembly_instances(records, this)?;
        let mut list = list_of(records, this)?;
        for i in instances {
            if let Err(
                e @ (crate::error::SolidError::InvalidInput { .. }
                | crate::error::SolidError::PortInvariant { .. }),
            ) = list.remove(this, records, i)
            {
                return Err(e);
            }
        }
        list.destroy();
        store_list(records, this, list)?;
        destroy_topology(records, this)
    }
}

/// A placed use of a component in an assembly (`SmAssemblyInstance`). Its parent
/// assembly is its list owner.
#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyInstance {
    /// `m_pName` (`None` for a null name).
    name: Option<String>,
    /// `m_vPlacement`.
    placement: Axis2Placement,
    /// `m_vScale`.
    scale: Vec3,
    /// `m_bMirrored`.
    mirrored: bool,
    /// `m_pComponent`: the B-rep or sub-assembly used.
    component: Option<ObjectKey>,
}

impl AssemblyInstance {
    /// `SmAssemblyInstance(pName, rOwningAssembly, pComponent, crPlacement, crScaling,
    /// bMirrored)`: the instance `this` is added at the end of `owning_assembly`'s
    /// list.
    ///
    /// # Errors
    ///
    /// Those of [`Assembly::add_instance`].
    pub fn new<R: OwningRecords + ?Sized>(
        records: &mut R,
        this: ObjectKey,
        name: Option<&str>,
        owning_assembly: ObjectKey,
        component: Option<ObjectKey>,
        placement: Axis2Placement,
        scale: Vec3,
        mirrored: bool,
    ) -> Result<Self> {
        Assembly::add_instance(records, owning_assembly, this)?;
        Ok(Self {
            name: name.map(str::to_owned),
            placement,
            scale,
            mirrored,
            component,
        })
    }

    /// The name (`GetName`).
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The placement (`GetPlacement`).
    #[must_use]
    pub const fn placement(&self) -> &Axis2Placement {
        &self.placement
    }

    /// The scale (`GetScale`).
    #[must_use]
    pub const fn scale(&self) -> Vec3 {
        self.scale
    }

    /// Whether the component is mirrored (`GetMirrored`).
    #[must_use]
    pub const fn mirrored(&self) -> bool {
        self.mirrored
    }

    /// The component (`GetComponent`).
    #[must_use]
    pub const fn component(&self) -> Option<ObjectKey> {
        self.component
    }

    /// The assembly whose list holds the instance `this` (`GetParentAssembly`: the
    /// list owner).
    ///
    /// # Errors
    ///
    /// [`crate::SolidError::InvalidInput`] when `this` has no record.
    pub fn parent_assembly<R: OwningRecords + ?Sized>(
        records: &R,
        this: ObjectKey,
    ) -> Result<Option<ObjectKey>> {
        records
            .topology(this)
            .map(crate::topology::topology::Topology::owner)
            .ok_or(crate::error::SolidError::invalid_input(
                OP,
                "object has no record",
            ))
    }

    /// The destructor `~SmAssemblyInstance` of the instance `this`: its parent
    /// assembly removes it (`RemoveInstance`), then its own list is forgotten
    /// (`~SmOwningTopology`) and `~SmTopology` runs (the instance is no longer listed).
    ///
    /// # Errors
    ///
    /// [`crate::SolidError::InvalidInput`] for missing records or list data.
    pub fn destroy<R: OwningRecords + ?Sized>(
        self,
        records: &mut R,
        this: ObjectKey,
    ) -> Result<()> {
        if let Some(parent) = Self::parent_assembly(records, this)? {
            Assembly::remove_instance(records, parent, this)?;
        }
        if let Some(mut list) = records.owning(this) {
            list.destroy();
            store_list(records, this, list)?;
        }
        destroy_topology(records, this)
    }
}
