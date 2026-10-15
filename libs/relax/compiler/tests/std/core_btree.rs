use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Included, Excluded, Unbounded};
fn random(s: &mut u32) -> u32 { *s ^= *s << 13; *s ^= *s >> 17; *s ^= *s << 5; *s }
#[test]
fn a01_splits_removes() {
    let mut m=BTreeMap::new(); let mut s=89173u32;
    for _ in 0..6000 { let k=random(&mut s)%2000; m.insert(k,k*3); }
    let mut sum=0u64;
    for _ in 0..3000 { let k=random(&mut s)%2000; if let Some(v)=m.remove(&k) { sum+=v as u64; } }
    println!("| tree {} {} {:?} {:?}",m.len(),sum,m.first_key_value(),m.last_key_value());
    let mut n=0; while let Some((k,v))=m.pop_first() { n+=k+v; if let Some((k,v))=m.pop_last() { n+=k+v; } }
    println!("| drained {} {}",n,m.len());
}
#[test]
fn a02_ranges_entries() {
    let mut m=BTreeMap::new(); for k in 0..180 { m.insert(k,k*2); }
    for k in 90..230 { *m.entry(k).and_modify(|v| *v+=1).or_insert(7)+=2; }
    let mut sum=0; { let mut it=m.range_mut((Excluded(23),Included(191))); loop { match it.next() { Some((k,v))=>{*v+=1; sum+=k+*v;},None=>break } match it.next_back() {Some((k,v))=>{*v+=2;sum+=k+*v;},None=>break} } }
    println!("| range {} {}",sum,m.range((Included(50),Excluded(50))).count());
    let mut sum=0; for (k,v) in m.range((Unbounded,Excluded(100))) { sum+=k+v; }
    println!("| bounded {}",sum);
    let mut it=m.iter(); let mut sum=0; while let Some((k,v))=it.next() {sum+=k+v;if let Some((k,v))=it.next_back(){sum+=k+v;}}
    println!("| mixed {}",sum);
    m.retain(|k,v| {*v+=1;k%3==0}); let mut r=m.split_off(&100); r.insert(90,999); m.append(&mut r);
    println!("| retained {:?}",m);
    let mut it=m.clone().into_iter(); println!("| owned {:?} {:?} {}",it.next(),it.next_back(),it.len());
    println!("| entries {:?} {:?}",m.first_entry().map(|e|e.remove_entry()),m.last_entry().map(|e|e.remove_entry()));
}
#[test]
fn a03_set_strings() {
    let mut s=BTreeSet::new(); for k in 0..300 {s.insert(k%171);}
    s.retain(|k|k%4!=0); let mut r=s.split_off(&90); s.append(&mut r);
    println!("| set {} {:?} {:?} {}",s.len(),s.pop_first(),s.pop_last(),s.range(30..=90).count());
    let mut m=BTreeMap::new(); for k in 0..90 {m.insert(format!("key{:03}",k),format!("value{}",k));}
    let removed=m.remove_entry("key061"); println!("| string {:?} {:?}",m.get("key031"),removed);
    let mut it=m.into_iter(); println!("| string ends {:?} {:?}",it.next(),it.next_back());
}

struct Dropped { count: std::rc::Rc<std::cell::Cell<usize>> }
impl Drop for Dropped { fn drop(&mut self) { self.count.set(self.count.get()+1); } }
#[test]
fn a04_drop_ownership() {
    let count=std::rc::Rc::new(std::cell::Cell::new(0usize));
    {
        let mut m=BTreeMap::new();
        for k in 0..700 { m.insert(k,Dropped{count:count.clone()}); }
        for k in 0..100 { m.insert(k,Dropped{count:count.clone()}); }
        m.retain(|k,_|k%3!=0);
        let mut r=m.split_off(&400); m.append(&mut r);
        let mut it=m.into_iter();
        for _ in 0..40 { drop(it.next()); drop(it.next_back()); }
        println!("| partial drops {}",count.get());
    }
    println!("| all drops {}",count.get());
}
