// std-core differential test (core_diff.sh): HashMap/HashSet/BTreeMap/VecDeque/Rc/RefCell/Box.
// Hash iteration order differs from real std by design, so only order-free results print.
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

#[test]
fn a01_hashmap() {
    let mut m = HashMap::new();
    let mut i = 0u32;
    while i < 1000 {
        m.insert(i % 97, i);
        i += 1;
    }
    println!("| len {}", m.len());
    println!("| get {} {}", m.get(&5).copied().unwrap_or(0), m.get(&500).is_none());
    println!("| contains {}", m.contains_key(&96));
    println!("| remove {}", m.remove(&5).unwrap());
    *m.entry(7).or_insert(0) += 1;
    *m.entry(500).or_insert(10) += 1;
    println!("| entry {} {}", m.get(&7).copied().unwrap(), m.get(&500).copied().unwrap());
    let mut sum = 0u64;
    for (k, v) in m.iter() {
        sum += (*k as u64) * 1000 + *v as u64;
    }
    println!("| sum {}", sum);
    m.retain(|k, _v| k % 2 == 0);
    println!("| retained {}", m.len());
}

#[test]
fn a02_hashset_btree() {
    let mut s = HashSet::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split(' ') {
        s.insert(w);
    }
    println!("| set {} {} {}", s.len(), s.contains("fox"), s.contains("cat"));
    let mut b = BTreeMap::new();
    for w in "the quick brown fox jumps over the lazy dog the end".split(' ') {
        *b.entry(w).or_insert(0u32) += 1;
    }
    for (k, v) in b.iter() {
        println!("| btree {} {}", k, v);
    }
    println!("| first {}", b.keys().next().unwrap());
}

#[test]
fn a03_vecdeque() {
    let mut d = VecDeque::new();
    let mut i = 0u32;
    while i < 10 {
        if i % 2 == 0 {
            d.push_back(i);
        } else {
            d.push_front(i);
        }
        i += 1;
    }
    for x in d.iter() {
        print!("| {}", x);
    }
    println!();
    println!("| front {} back {}", d.pop_front().unwrap(), d.pop_back().unwrap());
    println!("| len {} idx {}", d.len(), d[2]);
}

#[test]
fn a04_rc_refcell() {
    let shared = Rc::new(RefCell::new(Vec::new()));
    let other = shared.clone();
    shared.borrow_mut().push(3u32);
    other.borrow_mut().push(4);
    println!("| rc {} {}", Rc::strong_count(&shared), shared.borrow().len());
    let c = Cell::new(5u32);
    c.set(c.get() * 2);
    println!("| cell {}", c.get());
    let w = Rc::downgrade(&shared);
    drop(other);
    println!("| weak {} {}", w.upgrade().is_some(), Rc::strong_count(&shared));
    drop(shared);
    println!("| gone {}", w.upgrade().is_none());
    let b = Box::new(41u64);
    println!("| box {}", *b + 1);
}

#[test]
fn a05_linked_list() {
    use std::collections::LinkedList;
    let mut list = LinkedList::new();
    *list.push_back_mut(2) += 1;
    *list.push_front_mut(0) += 1;
    list.extend([4, 5, 6]);
    let mut it = list.extract_if(|x| {
        *x += 1;
        *x % 2 == 0
    });
    println!("| first {:?} {:?}", it.size_hint(), it.next());
    println!("| next {:?} {:?}", it, it.size_hint());
    drop(it);
    println!("| retained {:?} {}", list, list.len());
    let evens: Vec<_> = list.extract_if(|x| *x % 2 == 0).collect();
    println!("| removed {:?}, remaining {:?}", evens, list.iter().rev().copied().collect::<Vec<_>>());
    println!("| drain {:?} {}", list.extract_if(|_| true).collect::<Vec<_>>(), list.len());
    *list.push_front_mut(7) += 1;
    println!("| reused {:?}", list.pop_back());

    let mut words = LinkedList::from([String::from("keep"), String::from("take")]);
    let removed = words.extract_if(|word| {
        word.push('!');
        word == "take!"
    }).next();
    println!("| owned {:?} {:?}", removed, words);
}
