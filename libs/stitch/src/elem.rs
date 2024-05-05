use crate::{
    downcast::{DowncastMut, DowncastRef},
    extern_ref::UnguardedExternRef,
    func_ref::UnguardedFuncRef,
    ref_::RefType,
    store::{Handle, Store, StoreId, UnguardedHandle},
};

/// A Wasm element segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub(crate) struct Elem(pub(crate) Handle<ElemEntity>);

impl Elem {
    pub(crate) unsafe fn new_unguarded(store: &mut Store, elems: UnguardedElems) -> Self {
        Self(store.insert_elem(match elems {
            UnguardedElems::FuncRef(elems) => ElemEntity::FuncRef(ElemEntityT {
                elems: elems.into(),
            }),
            UnguardedElems::ExternRef(elems) => ElemEntity::ExternRef(ElemEntityT {
                elems: elems.into(),
            }),
        }))
    }

    pub(crate) fn type_(self, store: &Store) -> RefType {
        match self.0.as_ref(store) {
            ElemEntity::FuncRef(_) => RefType::FuncRef,
            ElemEntity::ExternRef(_) => RefType::ExternRef,
        }
    }

    pub(crate) fn drop_elems(self, store: &mut Store) {
        match self.0.as_mut(store) {
            ElemEntity::FuncRef(elem) => elem.drop_elems(),
            ElemEntity::ExternRef(elem) => elem.drop_elems(),
        }
    }

    pub(crate) unsafe fn from_unguarded(elem: UnguardedElem, store_id: StoreId) -> Self {
        Self(Handle::from_unguarded(elem, store_id))
    }

    pub(crate) fn to_unguarded(self, store_id: StoreId) -> UnguardedElem {
        self.0.to_unguarded(store_id).into()
    }
}

pub(crate) type UnguardedElem = UnguardedHandle<ElemEntity>;

#[derive(Clone, Debug)]
pub(crate) enum UnguardedElems {
    FuncRef(Vec<UnguardedFuncRef>),
    ExternRef(Vec<UnguardedExternRef>),
}

#[derive(Debug)]
pub(crate) enum ElemEntity {
    FuncRef(ElemEntityT<UnguardedFuncRef>),
    ExternRef(ElemEntityT<UnguardedExternRef>),
}

impl ElemEntity {
    pub(crate) fn downcast_ref<T>(&self) -> Option<&ElemEntityT<T>>
    where
        ElemEntityT<T>: DowncastRef<Self>,
    {
        ElemEntityT::downcast_ref(self)
    }

    pub(crate) fn downcast_mut<T>(&mut self) -> Option<&mut ElemEntityT<T>>
    where
        ElemEntityT<T>: DowncastMut<Self>,
    {
        ElemEntityT::downcast_mut(self)
    }
}

#[derive(Debug)]
pub(crate) struct ElemEntityT<T> {
    elems: Box<[T]>,
}

impl<T> ElemEntityT<T> {
    pub(crate) fn elems(&self) -> &[T] {
        &self.elems
    }

    pub(crate) fn drop_elems(&mut self) {
        self.elems = Box::new([]);
    }
}

impl DowncastRef<ElemEntity> for ElemEntityT<UnguardedFuncRef> {
    fn downcast_ref(elem: &ElemEntity) -> Option<&Self> {
        match elem {
            ElemEntity::FuncRef(elem) => Some(elem),
            _ => None,
        }
    }
}

impl DowncastMut<ElemEntity> for ElemEntityT<UnguardedExternRef> {
    fn downcast_mut(elem: &mut ElemEntity) -> Option<&mut Self> {
        match elem {
            ElemEntity::ExternRef(elem) => Some(elem),
            _ => None,
        }
    }
}

impl DowncastRef<ElemEntity> for ElemEntityT<UnguardedExternRef> {
    fn downcast_ref(elem: &ElemEntity) -> Option<&Self> {
        match elem {
            ElemEntity::ExternRef(elem) => Some(elem),
            _ => None,
        }
    }
}

impl DowncastMut<ElemEntity> for ElemEntityT<UnguardedFuncRef> {
    fn downcast_mut(elem: &mut ElemEntity) -> Option<&mut Self> {
        match elem {
            ElemEntity::FuncRef(elem) => Some(elem),
            _ => None,
        }
    }
}
