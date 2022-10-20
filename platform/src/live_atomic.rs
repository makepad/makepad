#![allow(non_camel_case_types)]
use {
    std::fmt::{Formatter,Debug, Error},
    std::marker::PhantomData,
    std::{
        sync::Arc,
        sync::atomic::{AtomicU32, AtomicI64,  Ordering, AtomicBool},
    },
    crate::{
        live_traits::*,
        makepad_live_compiler::*,
        cx::Cx,
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

impl <T> Clone for U32A<T> where T: LiveAtomicU32Enum {
    fn clone(&self)->Self{ 
        let t = self.get();
        U32A(AtomicU32::new(t.as_u32()), PhantomData)
    }
}

impl<T> Debug for U32A<T> where T: LiveAtomicU32Enum + Debug{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error>{
        self.get().fmt(f)
    }
}

impl<T> LiveAtomic for U32A<T> where T: LiveApply + LiveNew + 'static + LiveAtomicU32Enum {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut value = T::new(cx);
        let index = value.apply(cx, apply_from, index, nodes);
        self.set(value);
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

impl<T> LiveRead for U32A<T> where T:LiveRead + LiveAtomicU32Enum{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        self.get().live_read_to(id, out);
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

impl<T> LiveRead for Arc<T> where T:LiveRead{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        (self as &T).live_read_to(id, out);
    }
}

pub trait AtomicGetSet<T> {
    fn get(&self) -> T;
    fn set(&self, val: T);
}



// atomic f32


pub struct f32a(AtomicU32);

impl Clone for f32a {
    fn clone(&self)->Self{ 
        f32a(AtomicU32::new(self.get().to_bits()))
    }
}

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
        self.set(val);
        index
    }
}

impl LiveHook for f32a {}
impl LiveApply for f32a {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Debug for f32a{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error>{
        self.get().fmt(f)
    }
}

impl LiveRead for f32a{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        self.get().live_read_to(id, out);
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

impl Clone for u32a {
    fn clone(&self)->Self{ 
        u32a(AtomicU32::new(self.get()))
    }
}


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

impl Debug for u32a{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error>{
        self.get().fmt(f)
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

impl LiveRead for u32a{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        self.get().live_read_to(id, out);
    }
}

// atomic i64


pub struct i64a(AtomicI64);

impl Clone for i64a {
    fn clone(&self)->Self{ 
        i64a(AtomicI64::new(self.get()))
    }
}


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

impl<T, const N:usize> LiveAtomic for [T;N]  where T: LiveAtomic {
    fn apply_atomic(&self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        // we can only apply from an Array
        if nodes[index].is_array(){
            let mut index = index + 1;
            let mut count = 0;
            loop{
                if nodes[index].is_close(){
                    index += 1;
                    break;
                }
                if count < self.len(){
                    index = self[count].apply_atomic(cx, from, index, nodes);
                    count += 1;
                }
                else{
                   index = nodes.skip_node(index)
                }
            }
            index
        }
        else{
            cx.apply_error_expected_array(live_error_origin!(), index, nodes);
            nodes.skip_node(index)
        }
    }
} 


impl LiveHook for i64a {}
impl LiveApply for i64a {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Debug for i64a{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error>{
        self.get().fmt(f)
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

impl LiveRead for i64a{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        self.get().live_read_to(id, out);
    }
}





// atomic u32


pub struct boola(AtomicBool);

impl Clone for boola {
    fn clone(&self)->Self{ 
        boola(AtomicBool::new(self.get()))
    }
}


impl AtomicGetSet<bool> for boola {
    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn set(&self, val: bool) {
        self.0.store(val, Ordering::Relaxed);
    }
}

impl LiveAtomic for boola {
    fn apply_atomic(&self, cx: &mut Cx, apply_from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let mut val = false;
        let index = val.apply(cx, apply_from, index, nodes);
        self.0.store(val, Ordering::Relaxed);
        index
    }
}

impl LiveHook for boola {}
impl LiveApply for boola {
    fn apply(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        self.apply_atomic(cx, from, index, nodes)
    }
}

impl Debug for boola{
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error>{
        self.get().fmt(f)
    }
}

impl Into<boola> for bool {
    fn into(self) -> boola {
        boola(AtomicBool::new(self))
    }
}

impl LiveNew for boola {
    fn new(_cx: &mut Cx) -> Self {
        Self (AtomicBool::new(false))
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        bool::live_type_info(_cx)
    }
}

impl LiveRead for boola{
    fn live_read_to(&self, id:LiveId, out:&mut Vec<LiveNode>){
        self.get().live_read_to(id, out);
    }
}
