// std-core differential test (core_diff.sh): iterator adapters and ranges.

#[test]
fn a01_ranges() {
    let mut s = 0u64;
    for i in 0..10u64 {
        s += i;
    }
    for i in (0..=10u64).rev() {
        s = s * 2 + i;
    }
    for i in (3..40u64).step_by(7) {
        s += i * 1000;
    }
    println!("| ranges {}", s);
    let r = 5..9;
    println!("| contains {} {} len {}", r.contains(&5), r.contains(&9), (5..9).len());

    fn ends<I: core::iter::FusedIterator + DoubleEndedIterator + ExactSizeIterator>(mut it: I)
    where
        I::Item: std::fmt::Debug,
    {
        let first = it.next();
        let last = it.next_back();
        let middle = it.next();
        println!("| ends {:?} {:?} {:?} len {} fused {:?} {:?}", first, last, middle, it.len(), it.next_back(), it.next());
    }

    let half = core::range::Range::from(2u16..5);
    let it: core::range::RangeIter<u16> = half.iter();
    println!("| half {:?} rev {:?} count {}", it.clone(), it.rev().collect::<Vec<_>>(), half.iter().count());
    ends(half.into_iter());

    let incl = core::range::RangeInclusive::from(253u8..=255);
    let it: core::range::RangeInclusiveIter<u8> = incl.iter();
    println!("| inclusive {:?} rev {:?} count {}", it.clone(), it.rev().collect::<Vec<_>>(), incl.iter().count());
    ends(incl.into_iter());

    println!("| exact lengths {} {} {} {} {} {}", core::range::Range::from(2usize..5).iter().len(),
        core::range::Range::from(8u8..9).iter().len(), core::range::Range::from(-3isize..0).iter().len(),
        core::range::Range::from(-4i8..2).iter().len(), core::range::Range::from(-1i16..4).iter().len(),
        core::range::RangeInclusive::from(i8::MIN..=i8::MAX).iter().len());

    let mut chars = core::range::Range::from('a'..'g').into_iter();
    let mut signed = core::range::RangeInclusive::from(-2i8..=4).into_iter();
    println!("| skips {:?} {:?} {:?} {:?}", chars.nth(1), chars.nth_back(1), signed.nth_back(2), signed.nth(1));
    println!("| ordered {:?} {:?} {:?} {}", chars.clone().min(), chars.max(), signed.clone().last(), signed.is_sorted());

    let old_half: core::range::legacy::Range<u16> = half.into();
    let old_incl: core::range::legacy::RangeInclusive<u8> = incl.into();
    let old_from: core::range::legacy::RangeFrom<i32> = core::range::RangeFrom::from(-3..).into();
    let old_to: core::range::legacy::RangeToInclusive<i32> = core::range::RangeToInclusive::from(..=7).into();
    println!("| legacy {:?} {:?} {:?} {:?}", old_half, old_incl, old_from, old_to);
    println!("| round trip {:?} {:?} {:?} {:?}", core::range::Range::from(old_half),
        core::range::RangeInclusive::from(old_incl), core::range::RangeFrom::from(old_from),
        core::range::RangeToInclusive::from(old_to));
    let mut single = 9..=9;
    single.next();
    let converted = std::panic::catch_unwind(|| core::range::RangeInclusive::from(single));
    println!("| exhausted conversion empty or panics {}", converted.map_or(true, |r| r.is_empty()));
    let mut exhausted = u8::MAX..=u8::MAX;
    exhausted.next();
    println!("| overflowed endpoint conversion panics {}", std::panic::catch_unwind(|| core::range::RangeInclusive::from(exhausted)).is_err());

    // a RangeFrom past its type's maximum wraps without overflow checks (this file's profile, as
    // `rustc -O`), the old one and the new one's iterator alike
    let mut from = core::range::RangeFrom::from(253u8..).into_iter();
    let it: core::range::RangeFromIter<i8> = core::range::RangeFrom::from(-2i8..).iter();
    println!("| from {:?} then {:?} hint {:?} nth {:?} {:?}", from.by_ref().take(3).collect::<Vec<_>>(), from.next(),
        from.size_hint(), it.clone().nth(3), it);
    let mut old = 254u8..;
    println!("| legacy from {:?} {:?} {:?} nth {:?}", old.next(), old.next(), old.next(), (i8::MAX..).nth(1));
}

#[test]
fn a02_adapters() {
    let v: Vec<u32> = (1..=20).collect();
    let evens: Vec<u32> = v.iter().copied().filter(|x| x % 2 == 0).collect();
    println!("| evens {} {}", evens.len(), evens[3]);
    let sq: u32 = v.iter().map(|x| x * x).take(5).sum();
    println!("| sq {}", sq);
    let sk: u32 = v.iter().skip(15).sum();
    println!("| skip {}", sk);
    let z: u32 = v.iter().zip(v.iter().skip(1)).map(|(a, b)| a * b).sum();
    println!("| zip {}", z);
    let ch: u32 = v.iter().take(2).chain(v.iter().skip(18)).sum();
    println!("| chain {}", ch);
    println!("| any {} all {}", v.iter().any(|x| *x == 7), v.iter().all(|x| *x < 20));
    println!("| pos {}", v.iter().position(|x| *x == 9).unwrap());
    println!("| find {}", v.iter().find(|x| **x > 13).unwrap());
    println!("| max {} min {}", v.iter().max().unwrap(), v.iter().min().unwrap());
    println!("| count {}", v.iter().filter(|x| **x % 3 == 0).count());
    let f = v.iter().fold(0u64, |a, x| a * 3 + *x as u64 % 7);
    println!("| fold {}", f);
    let fm: u32 = v.iter().filter_map(|x| if x % 4 == 0 { Some(x / 4) } else { None }).sum();
    println!("| filter_map {}", fm);
    let tw: u32 = v.iter().take_while(|x| **x < 6).sum();
    let sw: u32 = v.iter().skip_while(|x| **x < 16).sum();
    println!("| take_while {} skip_while {}", tw, sw);
    let last = v.iter().last().unwrap();
    println!("| last {} nth {}", last, v.iter().nth(4).unwrap());
    let mk = v.iter().max_by_key(|x| (**x * 7) % 10).unwrap();
    let mn = v.iter().min_by_key(|x| (**x * 7) % 10).unwrap();
    println!("| max_by_key {} min_by_key {}", mk, mn);
}

#[test]
fn a03_peekable_flat_map() {
    let v: Vec<u32> = (0..6).collect();
    let mut p = v.iter().peekable();
    let mut out = 0;
    while let Some(x) = p.next() {
        if let Some(n) = p.peek() {
            out = out * 10 + *x + **n;
        }
    }
    println!("| peek {}", out);
    let mut items = (1..3).peekable();
    let rejected = items.next_if_map(|n| Err::<u8, _>(n + 10));
    let accepted = items.next_if_map(|n| Ok(n * 2));
    println!("| next_if_map {:?} {:?}", rejected, accepted);
    let rejected = items.next_if_map_mut(|n| {
        *n += 20;
        None::<u8>
    });
    let accepted = items.next_if_map_mut(|n| Some(*n * 3));
    let end = items.next_if_map_mut(|_| Some(0));
    println!("| next_if_map_mut {:?} {:?} {:?}", rejected, accepted, end);
    // A rejected end stays peeked even when the underlying iterator can resume.
    let mut source = [None, Some(7)].into_iter();
    let mut intermittent = std::iter::from_fn(|| source.next().flatten()).peekable();
    let at_end = intermittent.next_if_map(Ok);
    let still_at_end = intermittent.next_if_map_mut(|v| Some(*v));
    println!("| peeked end {:?} {:?} {:?} {:?}", at_end, still_at_end, intermittent.next(), intermittent.next());
    let fm: u32 = v.iter().flat_map(|x| 0..*x).sum();
    println!("| flat_map {}", fm);
    let nested: Vec<Vec<u32>> = v.iter().map(|x| (0..*x).collect()).collect();
    let fl: u32 = nested.iter().flatten().sum();
    println!("| flatten {}", fl);
    let sc: Vec<u32> = v.iter().scan(0, |acc, x| { *acc += x; Some(*acc) }).collect();
    println!("| scan {} {}", sc.len(), sc[5]);
    let (a, b): (Vec<u32>, Vec<u32>) = v.iter().partition(|x| **x > 2);
    println!("| partition {} {}", a.len(), b.len());
}

#[test]
fn a04_option_result() {
    let a: Option<u32> = Some(4);
    let b: Option<u32> = None;
    println!("| map {} {}", a.map(|x| x + 1).unwrap_or(0), b.map(|x| x + 1).unwrap_or(0));
    println!("| and_then {}", a.and_then(|x| if x > 3 { Some(x * 2) } else { None }).unwrap());
    println!("| or {} {}", b.or(Some(9)).unwrap(), a.xor(b).is_some());
    println!("| filter {}", a.filter(|x| *x > 10).is_none());
    let r: Result<u32, u32> = Err(3);
    println!("| res {} {} {}", r.is_err(), r.unwrap_or(7), r.map_err(|e| e + 1).unwrap_err());
    let ok: Result<u32, u32> = Ok(5);
    println!("| ok {}", ok.map(|x| x * 3).unwrap());
    let all: Option<Vec<u32>> = (1..4).map(|x| if x > 0 { Some(x) } else { None }).collect();
    println!("| collect-opt {}", all.unwrap().len());
    let bad: Result<Vec<u32>, u32> = (1..4).map(|x| if x != 2 { Ok(x) } else { Err(x) }).collect();
    println!("| collect-res {}", bad.unwrap_err());
}

#[test]
fn a05_range_inclusive_iteration() {
    use std::ops::RangeBounds;

    let mut r = 1..=2u8;
    r.next();
    let second = r.next();
    println!("| second {:?} {:?} {} {} {} {}", second, r, r.start(), r.end(), r.is_empty(), r == (3..=2));
    let third = r.next();
    println!("| third {:?} {:?} {} {} {} {}", third, r, r.start(), r.end(), r.is_empty(), r == (3..=2));

    let mut high = 250..=255u8;
    while high.next().is_some() {}
    let mut low = 0..=5u8;
    while low.next_back().is_some() {}
    println!("| forward overflow {:?} {} {:?} {:?}", high, high.is_empty(), high.start_bound(), high.end_bound());
    println!("| backward overflow {:?} {} {:?} {:?}", low, low.is_empty(), low.start_bound(), low.end_bound());

    let full = u8::MIN..=u8::MAX;
    println!("| full count {} sum {} last {:?}", full.clone().count(),
        full.clone().map(|x| x as u32).sum::<u32>(), full.clone().last());
    let mut copy = full.clone();
    let mut visited = 0;
    for _ in copy.by_ref() {
        visited += 1;
    }
    println!("| full loop {} {:?} {} original {:?}", visited, copy, copy.is_empty(), full);

    let mut past = 3..=6u8;
    let mut exact = past.clone();
    let mut back = past.clone();
    let skipped = past.nth(4);
    let end = exact.nth(3);
    let start = back.nth_back(3);
    println!("| nth past {:?} {:?} at end {:?} {:?} back {:?} {:?}", skipped, past, end, exact, start, back);
    println!("| reversed {:?}", (3..=6u8).rev().collect::<Vec<_>>());

    let mut signed = i8::MAX - 1..=i8::MAX;
    let values = [signed.next(), signed.next(), signed.next()];
    println!("| signed edge {:?} {:?} {}", values, signed, signed.is_empty());
    let mut letters = 'a'..='c';
    let chars = [letters.next(), letters.next(), letters.next(), letters.next()];
    let mut char_max = char::MAX..=char::MAX;
    let mut char_min = char::MIN..=char::MIN;
    let upper = char_max.next();
    let lower = char_min.next_back();
    println!("| chars {:?} {:?} edges {:?} {:?} {:?} {:?}", chars, letters, upper, char_max, lower, char_min);

    let mut folding = 1..=10;
    let stopped = folding.try_fold(0, |a, x| if x > 5 { None } else { Some(a + x) });
    println!("| try_fold stopped {:?} {:?}", stopped, folding);
    let completed = folding.try_rfold(0, |a, x| Some(a + x));
    println!("| try_rfold completed {:?} {:?}", completed, folding);
    println!("| fold order {} {}", (1..=3).fold(0, |a, x| 10 * a + x), (1..=3).rfold(0, |a, x| 10 * a + x));
}
