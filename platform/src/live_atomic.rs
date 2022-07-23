#![allow(non_camel_case_types)]
pub use {
    std::marker::PhantomData,
    std::{
        sync::Arc,
        sync::atomic::{AtomicU32, AtomicI64, AtomicBool, Ordering},
        any::TypeId,
    },
    crate::{
        live_traits::*,
        makepad_live_compiler::*,
        event::Event,
        cx::Cx,
        state::{State, StateAction, Animate}
    }
};

pub trait LiveAtomicValue {
    fn apply_value_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveAtomic {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize;
}

pub trait LiveAtomicU32Enum {
    fn as_u32(&self) -> u32;
    fn from_u32(val: u32) -> Self;
}

// Atomic u32 enum template



pub struct U32A<T>(AtomicU32, PhantomData<T>) where T: LiveAtomicU32Enum;

impl <T> U32A<T> where T: LiveAtomicU32Enum {
    pub fn set(&self, val: T) {
        self.0.store(val.as_u32(), Ordering::Relaxed)
    }
    
    pub fn get(&self) -> T {
        T::from_u32(self.0.load(Ordering::Relaxed))
    }
}

impl<T> LiveAtomic for U32A<T> where T: LiveApply + LiveNew + 'static + LiveAtomicU32Enum {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut val = 0u32;
        let index = val.apply(cx, apply_from, index, nodes);
        self.0.store(val, Ordering::Relaxed);
        index
    }
}

impl<T> LiveHook for U32A<T> where T: LiveApply + LiveNew + 'static +  LiveAtomicU32Enum {}
impl<T> LiveApply for U32A<T> where T: LiveApply + LiveNew + 'static + LiveAtomicU32Enum {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl<T> LiveNew for U32A<T> where T: LiveApply + LiveNew + 'static +  LiveAtomicU32Enum {
    fn new(cx: &mut Cx) -> Self {
        Self (AtomicU32::new(T::new(cx).as_u32()), PhantomData)
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}
/*
impl Into<U32A<T>> for T where T: LiveApply + LiveNew + 'static + LiveAtomic + LiveAtomicU32Enum{
    fn into(self) -> U32A<T> {
        Self (AtomicU32::new(self.as_u32()), PhantomData)
    }
}
*/

// Arc



impl<T> LiveHook for Arc<T> where T: LiveApply + LiveNew + 'static + LiveAtomic {}
impl<T> LiveApply for Arc<T> where T: LiveApply + LiveNew + 'static + LiveAtomic {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl<T> LiveNew for Arc<T> where T: LiveApply + LiveNew + 'static + LiveAtomic {
    fn new(cx: &mut Cx) -> Self {
        Arc::new(T::new(cx))
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        T::live_type_info(_cx)
    }
}




pub trait AtomicGetSet<T> {
    fn get(&self) -> T;
    fn set(&self, val: T);
}


// atomic f32


pub struct f32a(AtomicU32);

impl AtomicGetSet<f32> for f32a {
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, val: f32) {
        self.0.store(val.to_bits(), Ordering::Relaxed);
    }
}

impl LiveAtomic for f32a {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut val = 0.0f32;
        let index = val.apply(cx, apply_from, index, nodes);
        self.0.store(val.to_bits(), Ordering::Relaxed);
        index
    }
}

impl LiveHook for f32a {}
impl LiveApply for f32a {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Into<f32a> for f32 {
    fn into(self) -> f32a {
        f32a(AtomicU32::new(self.to_bits()))
    }
}

impl LiveNew for f32a {
    fn new(_cx: &mut Cx) -> Self {
        Self (AtomicU32::new(0.0f32.to_bits()))
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        f32::live_type_info(_cx)
    }
}



// atomic u32


pub struct u32a(AtomicU32);

impl AtomicGetSet<u32> for u32a {
    fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, val: u32) {
        self.0.store(val, Ordering::Relaxed);
    }
}

impl LiveAtomic for u32a {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut val = 0u32;
        let index = val.apply(cx, apply_from, index, nodes);
        self.0.store(val, Ordering::Relaxed);
        index
    }
}

impl LiveHook for u32a {}
impl LiveApply for u32a {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Into<u32a> for u32 {
    fn into(self) -> u32a {
        u32a(AtomicU32::new(self))
    }
}

impl LiveNew for u32a {
    fn new(_cx: &mut Cx) -> Self {
        Self (AtomicU32::new(0))
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        u32::live_type_info(_cx)
    }
}


// atomic i64


pub struct i64a(AtomicI64);

impl AtomicGetSet<i64> for i64a {
    fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, val: i64) {
        self.0.store(val, Ordering::Relaxed);
    }
}

impl LiveAtomic for i64a {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut val = 0i64;
        let index = val.apply(cx, apply_from, index, nodes);
        self.0.store(val, Ordering::Relaxed);
        index
    }
}

impl LiveHook for i64a {}
impl LiveApply for i64a {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Into<i64a> for i64 {
    fn into(self) -> i64a {
        i64a(AtomicI64::new(self))
    }
}

impl LiveNew for i64a {
    fn new(_cx: &mut Cx) -> Self {
        Self (AtomicI64::new(0))
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        u32::live_type_info(_cx)
    }
}



