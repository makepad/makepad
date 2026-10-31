// makepad platform/src/thread.rs detect_machine_topology (linux): a cpu list "0-3,8,10-11" parsed by
// split(',').flat_map(|r| first.zip(last).into_iter().flat_map(|(a, b)| a..=b)). The old selector type-checked
// the Map/FlatMap::next instances of this chain in ~2 s each (FlatMap<IntoIter<(usize, usize)>,
// RangeInclusive<usize>, {closure}> under Split<char>); the N01 acceptance target is < 20 ms per instance.
use std::collections::BTreeSet;

fn cpu_list(list: &str) -> BTreeSet<usize> {
    list.trim()
        .split(',')
        .flat_map(|range| {
            let mut bounds = range.split('-');
            let first = bounds.next().and_then(|n| n.parse::<usize>().ok());
            let last = bounds.next().and_then(|n| n.parse::<usize>().ok()).or(first);
            first.zip(last).into_iter().flat_map(|(first, last)| first..=last)
        })
        .collect()
}

fn cpu_vec(list: &str) -> Vec<usize> {
    list.split(',')
        .flat_map(|range| {
            let mut bounds = range.split('-');
            let first = bounds.next().and_then(|n| n.parse::<usize>().ok());
            let last = bounds.next().and_then(|n| n.parse::<usize>().ok()).or(first);
            first.zip(last).into_iter().flat_map(|(a, b)| a..=b)
        })
        .collect()
}

#[test]
fn cpu_list_ranges_and_singletons() {
    let set = cpu_list(" 0-3,8,10-11\n");
    assert_eq!(set.into_iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 8, 10, 11]);
}

#[test]
fn cpu_list_into_vec_skips_garbage() {
    assert_eq!(cpu_vec("2-4,x,7,9-8"), vec![2, 3, 4, 7]);
    assert_eq!(cpu_vec(""), Vec::<usize>::new());
}
