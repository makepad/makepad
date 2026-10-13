//! Differential coverage for the byte-layout B-tree.
use relax_std_check::collections::{BTreeMap as Map, BTreeSet as Set};
use std::collections::{BTreeMap as Real, BTreeSet as RealSet};
use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::cell::Cell;
use std::rc::Rc;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn rand(s: &mut u64) -> u32 {
    *s ^= *s << 13; *s ^= *s >> 7; *s ^= *s << 17; (*s >> 16) as u32
}
fn bounds(s: &mut u64) -> (Bound<u32>, Bound<u32>) {
    let a = rand(s) % 512; let b = rand(s) % 512;
    let lo = a.min(b); let hi = a.max(b) + 1;
    let l = match rand(s) % 3 { 0 => Unbounded, 1 => Included(lo), _ => Excluded(lo) };
    let r = match rand(s) % 3 { 0 => Unbounded, 1 => Included(hi), _ => Excluded(hi) };
    (l, r)
}
fn same(a: &Map<u32, i64>, b: &Real<u32, i64>) {
    assert_eq!(a.len(), b.len());
    assert!(a.iter().eq(b.iter()));
    assert!(a.iter().rev().eq(b.iter().rev()));
    assert_eq!(a.first_key_value(), b.first_key_value());
    assert_eq!(a.last_key_value(), b.last_key_value());
}

#[test]
fn randomized_300k() {
    let mut a = Map::new(); let mut b = Real::new();
    let mut sa = Set::new(); let mut sb = RealSet::new();
    let mut seed = 0xdead_beef_8172_9311;
    for step in 0..300_000 {
        let k = rand(&mut seed) % 512; let v = rand(&mut seed) as i64;
        match rand(&mut seed) % 24 {
            0..=9 => { assert_eq!(a.insert(k, v), b.insert(k, v)); assert_eq!(sa.insert(k), sb.insert(k)); }
            10 => { assert_eq!(a.remove(&k), b.remove(&k)); assert_eq!(sa.remove(&k), sb.remove(&k)); }
            11 => { assert_eq!(a.get(&k), b.get(&k)); assert_eq!(sa.get(&k), sb.get(&k)); if let Some(x) = a.get_mut(&k) { *x ^= v; } if let Some(x) = b.get_mut(&k) { *x ^= v; } }
            12 => { *a.entry(k).and_modify(|x| *x += 1).or_insert(v) += 2; *b.entry(k).and_modify(|x| *x += 1).or_insert(v) += 2; }
            13 => { assert_eq!(a.remove_entry(&k), b.remove_entry(&k)); }
            14 => {
                let r = bounds(&mut seed);
                assert!(a.range(r).eq(b.range(r)));
                assert!(sa.range(r).eq(sb.range(r)));
                let mut ai = a.range_mut(r); let mut bi = b.range_mut(r);
                loop {
                    let rev = rand(&mut seed) & 1 == 0;
                    let x = if rev { ai.next_back() } else { ai.next() };
                    let y = if rev { bi.next_back() } else { bi.next() };
                    match (x, y) { (Some((k,x)), Some((j,y))) => { assert_eq!(k,j); assert_eq!(x,y); *x ^= 7; *y ^= 7; }, (None,None) => break, _ => panic!("range length") }
                }
                assert!(ai.next().is_none() && ai.next_back().is_none());
            }
            15 => {
                let mut ai = a.iter_mut(); let mut bi = b.iter_mut();
                loop {
                    let rev = rand(&mut seed) & 1 == 0;
                    let x = if rev { ai.next_back() } else { ai.next() };
                    let y = if rev { bi.next_back() } else { bi.next() };
                    match (x,y) { (Some((k,x)),Some((j,y))) => { assert_eq!(k,j); assert_eq!(x,y); *x += 1; *y += 1; }, (None,None) => break, _ => panic!("iter length") }
                }
            }
            16 => { assert_eq!(a.pop_first(), b.pop_first()); assert_eq!(sa.pop_first(), sb.pop_first()); }
            17 => { assert_eq!(a.pop_last(), b.pop_last()); assert_eq!(sa.pop_last(), sb.pop_last()); }
            18 => {
                a.retain(|k,v| { *v ^= 3; k % 31 != 0 }); b.retain(|k,v| { *v ^= 3; k % 31 != 0 });
                sa.retain(|k| k % 31 != 0); sb.retain(|k| k % 31 != 0);
            }
            19 => {
                let mut ar = a.split_off(&k); let mut br = b.split_off(&k); same(&ar,&br); same(&a,&b);
                // Duplicate keys in append: right values win.
                a.insert(k,v+1); b.insert(k,v+1); a.append(&mut ar); b.append(&mut br); assert!(ar.is_empty());
                let mut sr = sa.split_off(&k); let mut tr = sb.split_off(&k); assert!(sr.iter().eq(tr.iter())); sa.append(&mut sr); sb.append(&mut tr);
            }
            20 => { same(&a.clone(), &b.clone()); assert!(sa.clone().iter().eq(sb.iter())); }
            21 => {
                let mut ai = a.clone().into_iter(); let mut bi = b.clone().into_iter();
                for _ in 0..7 { assert_eq!(ai.next(),bi.next()); assert_eq!(ai.next_back(),bi.next_back()); }
            }
            22 => {
                if let Some(mut e) = a.first_entry() { *e.get_mut() ^= 11; }
                if let Some(mut e) = b.first_entry() { *e.get_mut() ^= 11; }
                if let Some(e) = a.last_entry() { assert_eq!(Some(e.remove_entry()), b.last_entry().map(|e| e.remove_entry())); }
            }
            _ => {
                let removed: Vec<_> = a.extract_if(|k,v| { *v += 1; k % 17 == 0 }).collect();
                let mut expected = Vec::new(); b.retain(|k,v| { *v += 1; if k % 17 == 0 { expected.push((*k,*v)); false } else { true } });
                assert_eq!(removed,expected);
            }
        }
        if step % 101 == 0 { same(&a,&b); assert!(sa.iter().eq(sb.iter())); }
    }
    same(&a,&b); assert!(sa.iter().eq(sb.iter()));
}

#[test]
fn deep_splits_and_merges() {
    let mut a = Map::new(); let mut b = Real::new(); let mut seed = 78123;
    for k in 0..30_000 { let v = rand(&mut seed) as i64; a.insert(k,v); b.insert(k,v); }
    same(&a,&b);
    for _ in 0..90_000 { let k = rand(&mut seed)%30_000; assert_eq!(a.remove_entry(&k), b.remove_entry(&k)); }
    same(&a,&b);
    while !b.is_empty() { assert_eq!(a.pop_first(),b.pop_first()); assert_eq!(a.pop_last(),b.pop_last()); }
    for k in (0..10_000).rev() { a.insert(k,k as i64); b.insert(k,k as i64); }
    a.retain(|k,_| k % 7 == 0); b.retain(|k,_| k % 7 == 0); same(&a,&b);
    a.retain(|_,_| false); assert!(a.is_empty());
}

#[test]
fn strings_300k() {
    let mut a = Map::new(); let mut b = Real::new(); let mut seed = 971321;
    for i in 0..300_000 {
        let key = format!("key{:03}", rand(&mut seed)%256);
        match rand(&mut seed)%8 {
            0..=3 => { let v = format!("value{i}"); assert_eq!(a.insert(key.clone(),v.clone()),b.insert(key,v)); }
            4 => { assert_eq!(a.remove_entry(key.as_str()),b.remove_entry(key.as_str())); }
            5 => { assert_eq!(a.get(key.as_str()),b.get(key.as_str())); }
            6 => { a.entry(key.clone()).and_modify(|v| v.push('x')).or_insert(key.clone()); b.entry(key.clone()).and_modify(|v|v.push('x')).or_insert(key); }
            _ => { let r = (Included(key.as_str()),Unbounded); assert!(a.range::<str,_>(r).eq(b.range::<str,_>(r))); }
        }
        if i % 503 == 0 { assert!(a.iter().eq(b.iter())); assert!(a.clone().into_iter().eq(b.clone())); }
    }
}

#[derive(Debug)]
struct Value { id: usize, counts: Rc<Vec<Cell<usize>>> }
impl Drop for Value { fn drop(&mut self) { let x=&self.counts[self.id]; x.set(x.get()+1); assert_eq!(x.get(),1,"double drop {}",self.id); } }
fn value(i: usize,c: &Rc<Vec<Cell<usize>>>) -> Value { Value { id:i, counts:c.clone() } }
#[test]
fn drops_and_partial_iterators() {
    let counts = Rc::new((0..12_000).map(|_|Cell::new(0)).collect::<Vec<_>>());
    {
        let mut a = Map::new();
        for i in 0..10_000 { a.insert(i,value(i,&counts)); }
        for i in 0..1000 { a.insert(i,value(10_000+i,&counts)); }
        for i in 1000..2000 { drop(a.remove(&i)); }
        a.retain(|k,_| k%3!=0);
        { let mut e=a.extract_if(|k,_| k%2==0); for _ in 0..17 { drop(e.next()); } }
        let mut r=a.split_off(&7000); a.append(&mut r);
        for i in 0..1000 { a.insert(i,value(11_000+i,&counts)); }
        let mut it=a.into_iter(); for _ in 0..100 { drop(it.next()); drop(it.next_back()); }
    }
    assert!(counts.iter().all(|c|c.get()==1));
}

#[derive(Debug)]
struct Key { n: u32, fuel: Rc<Cell<i32>>, dropped: Rc<Cell<usize>> }
impl Drop for Key { fn drop(&mut self) { self.dropped.set(self.dropped.get()+1); } }
impl PartialEq for Key { fn eq(&self,o:&Self)->bool { self.n==o.n } }
impl Eq for Key {}
impl PartialOrd for Key { fn partial_cmp(&self,o:&Self)->Option<std::cmp::Ordering> { Some(self.cmp(o)) } }
impl Ord for Key { fn cmp(&self,o:&Self)->std::cmp::Ordering { let n=self.fuel.get(); if n==0 { panic!("comparison panic"); } self.fuel.set(n-1); self.n.cmp(&o.n) } }
#[test]
fn comparator_panic_owns_everything_once() {
    for limit in 0..20 {
        let fuel=Rc::new(Cell::new(1_000_000)); let dropped=Rc::new(Cell::new(0));
        let counts=Rc::new((0..201).map(|_|Cell::new(0)).collect::<Vec<_>>());
        let mut a=Map::new();
        for i in 0..200 { a.insert(Key{n:i*2,fuel:fuel.clone(),dropped:dropped.clone()},value(i as usize,&counts)); }
        fuel.set(limit);
        let _=catch_unwind(AssertUnwindSafe(|| a.insert(Key{n:201,fuel:fuel.clone(),dropped:dropped.clone()},value(200,&counts))));
        fuel.set(1_000_000); assert!(a.iter().map(|(k,_)|k.n).is_sorted());
        drop(a); assert_eq!(dropped.get(),201); assert!(counts.iter().all(|c|c.get()==1));
    }
}

#[repr(align(256))]
#[derive(PartialEq,Eq,PartialOrd,Ord,Clone,Debug)]
struct Aligned(u32);
#[test]
fn alignment_zsts_and_live_mut_refs() {
    let mut a=Map::new(); for i in 0..1000 { a.insert(Aligned(i),Aligned(i+1)); }
    let mut it=a.iter_mut(); let (k,x)=it.next().unwrap(); let (_,y)=it.next_back().unwrap();
    assert_eq!(k as *const Aligned as usize % 256,0); assert_eq!(x as *mut Aligned as usize % 256,0);
    x.0+=2; y.0+=3; assert_eq!(x.0,3); assert_eq!(y.0,1003);
    let rest: Vec<_>=it.collect(); for (_,x) in rest { x.0+=1; }
    let mut z=Map::new(); z.insert((),()); assert_eq!(z.pop_first(),Some(((),())));
    let mut z=Map::new(); for i in 0..1000 { z.insert(i,()); } assert_eq!(z.into_values().count(),1000);
}

fn panic_text(e: Box<dyn std::any::Any+Send>) -> String {
    if let Some(s)=e.downcast_ref::<String>() { s.clone() } else { e.downcast_ref::<&str>().unwrap().to_string() }
}
#[test]
fn range_panics_and_bounds() {
    let mut a=Map::new(); let mut b=Real::new(); for k in 0..20 { a.insert(k,k); b.insert(k,k); }
    let all=[Unbounded,Included(0),Excluded(0),Included(10),Excluded(10),Included(30),Excluded(30)];
    for l in all { for r in all {
        let x=catch_unwind(AssertUnwindSafe(||a.range((l,r)).map(|(k,v)|(*k,*v)).collect::<Vec<_>>()));
        let y=catch_unwind(AssertUnwindSafe(||b.range((l,r)).map(|(k,v)|(*k,*v)).collect::<Vec<_>>()));
        match (x,y) { (Ok(x),Ok(y))=>assert_eq!(x,y), (Err(x),Err(y))=>assert_eq!(panic_text(x),panic_text(y)), _=>panic!("panic mismatch") }
    } }
}

#[derive(Debug)]
struct PanicDrop { id: usize, counts: Rc<Vec<Cell<usize>>>, panic: Rc<Cell<bool>> }
impl Drop for PanicDrop {
    fn drop(&mut self) {
        self.counts[self.id].set(self.counts[self.id].get()+1);
        if self.id==31 && self.panic.replace(false) { panic!("drop panic"); }
    }
}
#[test]
fn destructor_panic_finishes_drain() {
    let counts=Rc::new((0..1000).map(|_|Cell::new(0)).collect::<Vec<_>>());
    let panic=Rc::new(Cell::new(true)); let mut a=Map::new();
    for id in 0..1000 { a.insert(id,PanicDrop{id,counts:counts.clone(),panic:panic.clone()}); }
    assert!(catch_unwind(AssertUnwindSafe(||a.clear())).is_err());
    assert!(a.is_empty()); assert!(counts.iter().all(|x|x.get()==1));
}

#[test]
#[ignore = "optional isolated optimized timing; best of five"]
fn timing_best_of_five() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut seed=42; let keys: Vec<u64>=(0..100_000).map(|_| { rand(&mut seed); seed }).collect();
    let mut port=u128::MAX; let mut real=u128::MAX;
    for _ in 0..5 {
        let start=Instant::now(); let mut m=Map::new();
        for (i,k) in keys.iter().enumerate() { m.insert(*k,i as u64); }
        let mut sum=0u64;
        for k in &keys { sum=sum.wrapping_add(*m.get(k).unwrap()); }
        for k in keys.iter().take(1000) { for (_,v) in m.range(*k..k.saturating_add(u64::MAX/1000)) {sum=sum.wrapping_add(*v);} }
        for k in &keys { sum=sum.wrapping_add(m.remove(k).unwrap()); }
        black_box(sum); port=port.min(start.elapsed().as_nanos());
        let start=Instant::now(); let mut m=Real::new();
        for (i,k) in keys.iter().enumerate() { m.insert(*k,i as u64); }
        let mut other=0u64;
        for k in &keys { other=other.wrapping_add(*m.get(k).unwrap()); }
        for k in keys.iter().take(1000) { for (_,v) in m.range(*k..k.saturating_add(u64::MAX/1000)) {other=other.wrapping_add(*v);} }
        for k in &keys { other=other.wrapping_add(m.remove(k).unwrap()); }
        black_box(other); real=real.min(start.elapsed().as_nanos()); assert_eq!(sum,other);
    }
    println!("btree best/5: port {:.3} ms, real {:.3} ms, ratio {:.3}",port as f64/1e6,real as f64/1e6,port as f64/real as f64);
}

struct CloneValue {
    live: Rc<Cell<usize>>,
    fuel: Rc<Cell<usize>>,
}
impl Clone for CloneValue {
    fn clone(&self) -> Self {
        let fuel = self.fuel.get();
        if fuel == 0 { panic!("clone panic"); }
        self.fuel.set(fuel-1);
        self.live.set(self.live.get()+1);
        Self { live:self.live.clone(), fuel:self.fuel.clone() }
    }
}
impl Drop for CloneValue { fn drop(&mut self) { self.live.set(self.live.get()-1); } }
#[test]
fn clone_panic_and_entry_removal() {
    let live=Rc::new(Cell::new(1000)); let fuel=Rc::new(Cell::new(73));
    let mut a=Map::new();
    for k in 0..1000 { a.insert(k,CloneValue { live:live.clone(),fuel:fuel.clone() }); }
    assert!(catch_unwind(AssertUnwindSafe(||a.clone())).is_err());
    assert_eq!(live.get(),1000);
    if let relax_std_check::collections::btree_map::Entry::Occupied(e)=a.entry(500) {
        assert_eq!(e.remove_entry().0,500);
    } else { panic!("missing occupied entry"); }
    assert_eq!(live.get(),999);
    fuel.set(1000); let b=a.clone(); assert_eq!(live.get(),1998); drop(b);
    a.clear(); assert_eq!(live.get(),0);
}

#[test]
fn range_validation_only_with_root() {
    std::panic::set_hook(Box::new(|_| {}));
    // never allocated, allocated then emptied by removal (root leaf stays), cleared
    for state in 0..3 {
        let mut h: Map<u32, u32> = Map::new();
        let mut r: Real<u32, u32> = Real::new();
        if state >= 1 {
            h.insert(1, 1);
            r.insert(1, 1);
            h.remove(&1);
            r.remove(&1);
        }
        if state == 2 {
            h.clear();
            r.clear();
        }
        let hp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.range((Excluded(3), Excluded(3))).count())).is_err();
        let rp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.range((Excluded(3), Excluded(3))).count())).is_err();
        assert_eq!(hp, rp, "state {}", state);
        let hp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.range((Included(5), Included(3))).count())).is_err();
        let rp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| r.range((Included(5), Included(3))).count())).is_err();
        assert_eq!(hp, rp, "state {} (start > end)", state);
    }
    let _ = std::panic::take_hook();
}
