// Port of box3d/test/test_id.c

use makepad_box3d::ensure;
use makepad_box3d::id::*;

#[test]
fn id_test() {
    let x: u64 = 0x0123456789ABCDEF;

    {
        let id = load_body_id(x);
        let y = store_body_id(id);
        ensure!(x == y);
    }

    {
        let id = load_shape_id(x);
        let y = store_shape_id(id);
        ensure!(x == y);
    }

    {
        let id = load_joint_id(x);
        let y = store_joint_id(id);
        ensure!(x == y);
    }
}
