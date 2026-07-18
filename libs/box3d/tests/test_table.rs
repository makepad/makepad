// Port of box3d/test/test_table.c
// The debug probe-count instrumentation is not ported.

use makepad_box3d::ctz::{bounding_power_of_2, round_up_power_of_2};
use makepad_box3d::ensure;
use makepad_box3d::table::*;
use makepad_box3d::timer::{get_milliseconds, get_ticks};

const SET_SPAN: i32 = 317;
const ITEM_COUNT: usize = ((SET_SPAN * SET_SPAN - SET_SPAN) / 2) as usize;

#[test]
fn table_test() {
    let power = bounding_power_of_2(3008);
    ensure!(power == 12);

    let next_power_of_2 = round_up_power_of_2(3008);
    ensure!(next_power_of_2 == (1 << power));

    let n = SET_SPAN;
    let item_count = ITEM_COUNT as u32;
    let mut removed = vec![false; ITEM_COUNT];

    for _iter in 0..1 {
        let mut set = create_set(16);

        // Fill set
        for i in 0..n {
            for j in (i + 1)..n {
                let key = shape_pair_key(i, j, 0);
                add_key(&mut set, key);
            }
        }

        ensure!(set.count == item_count);

        // Remove a portion of the set
        let mut k = 0usize;
        let mut remove_count = 0u32;
        for i in 0..n {
            for j in (i + 1)..n {
                if j == i + 1 {
                    let key = shape_pair_key(i, j, 0);
                    remove_key(&mut set, key);
                    removed[k] = true;
                    k += 1;
                    remove_count += 1;
                } else {
                    removed[k] = false;
                    k += 1;
                }
            }
        }

        ensure!(set.count == (item_count - remove_count));

        // Test key search
        let ticks = get_ticks();

        let mut k = 0usize;
        for i in 0..n {
            for j in (i + 1)..n {
                let key = shape_pair_key(j, i, 0);
                ensure!(contains_key(&set, key) || removed[k]);
                k += 1;
            }
        }

        let ms = get_milliseconds(ticks);
        println!(
            "set: count = {}, contains_key = {:.5} ms, ave = {:.5} us",
            item_count,
            ms,
            1000.0 * ms / item_count as f32
        );

        // Remove all keys from set
        for i in 0..n {
            for j in (i + 1)..n {
                let key = shape_pair_key(i, j, 0);
                remove_key(&mut set, key);
            }
        }

        ensure!(set.count == 0);

        destroy_set(&mut set);
    }
}
