// Cell's slice/array API: from_mut, as_slice_of_cells, as_array_of_cells (zune-inflate's
// copy_rep_matches pattern: overlapping copies through a slice of cells), update, UnsafeCell::from_mut.
use std::cell::{Cell, UnsafeCell};

#[test]
fn a01_slice_of_cells() {
    let mut data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    {
        let cells = Cell::from_mut(&mut data[..]).as_slice_of_cells();
        // overlapping forward copy: dist 3, 6 bytes from position 3
        for i in 0..6 {
            cells[3 + i].set(cells[i].get());
        }
        cells[0].swap(&cells[9]);
        println!("| len {} first {} last {}", cells.len(), cells[0].get(), cells[9].get());
    }
    println!("| data {:?}", data);
}

#[test]
fn a02_array_of_cells() {
    let mut arr = [10i32, 20, 30, 40];
    let cells: &[Cell<i32>; 4] = Cell::from_mut(&mut arr).as_array_of_cells();
    for c in cells.iter() {
        c.update(|v| v * 2 + 1);
    }
    cells[1].replace(-5);
    println!("| cells {:?}", cells.iter().map(|c| c.get()).collect::<Vec<_>>());
    println!("| arr {:?}", arr);
}

#[test]
fn a03_update_and_unsafecell() {
    let c = Cell::new(5u32);
    let r: () = c.update(|x| x + 7);
    println!("| update {:?} {}", r, c.get());
    let mut v = 9u64;
    let u: &mut UnsafeCell<u64> = UnsafeCell::from_mut(&mut v);
    *u.get_mut() += 1;
    unsafe { *u.get() *= 3 };
    println!("| unsafecell {}", v);
    let mut empty: [u16; 0] = [];
    println!("| empty {}", Cell::from_mut(&mut empty[..]).as_slice_of_cells().len());
}

#[test]
fn a04_no_niche() {
    // UnsafeCell hides its value's niche (rustc_layout_no_niche): Option needs its own tag
    println!("| option cell ref {} option cell nonzero {}", std::mem::size_of::<Option<UnsafeCell<&u8>>>(), std::mem::size_of::<Option<Cell<std::num::NonZeroU32>>>());
}
