use std::marker::PhantomData;
use std::ops::Deref;

/// Contains handles of modified objects.
///
/// This is a wrapper over a `Vec` to ensure we don’t forget to set the object’s
/// MODIFIED flag when adding it to this set.
/// It is possible to bypass the wrapper with `.as_mut_internal`. But this should only
/// be done for internal engine usage (like the physics pipeline).
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
pub struct ModifiedObjects<Handle, Object>(Vec<Handle>, PhantomData<Object>);

impl<Handle, Object> Default for ModifiedObjects<Handle, Object> {
    fn default() -> Self {
        Self(Vec::new(), PhantomData)
    }
}

/// Objects that internally track a flag indicating whether they've been modified
pub trait HasModifiedFlag {
    /// Whether the object has been modified
    fn has_modified_flag(&self) -> bool;
    /// Mark object as modified
    fn set_modified_flag(&mut self);
}

impl<Handle, Object> Deref for ModifiedObjects<Handle, Object> {
    type Target = Vec<Handle>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<Handle, Object: HasModifiedFlag> ModifiedObjects<Handle, Object> {
    /// Preallocate memory for `capacity` handles
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity), PhantomData)
    }

    /// Remove every handle from this set.
    ///
    /// Note that the corresponding object MODIFIED flags won’t be reset automatically by this function.
    pub fn clear(&mut self) {
        self.0.clear()
    }

    /// Pushes a object handle to this set after checking that it doesn’t have the MODIFIED
    /// flag set.
    ///
    /// This will also set the object’s MODIFIED flag.
    pub fn push_once(&mut self, handle: Handle, object: &mut Object) {
        if !object.has_modified_flag() {
            self.push_unchecked(handle, object);
        }
    }

    /// Pushes an object handle to this set without checking if the object already has the MODIFIED
    /// flags.
    ///
    /// Only use in situation where you are certain (due to other contextual information) that
    /// the object isn’t already in the set.
    ///
    /// This will also set the object’s MODIFIED flag.
    pub fn push_unchecked(&mut self, handle: Handle, object: &mut Object) {
        object.set_modified_flag();
        self.0.push(handle);
    }
}
