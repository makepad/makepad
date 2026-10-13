// M1 off-lock: a patch's optimisation and code generation run with the unit lock released.
// A program thread keeps calling functions it never called before (each first call is a lazy
// compile, which needs the unit lock) while an edit makes `heavy` reachable (~250 ms to build
// on node 165): no first call may wait more than 100 ms. (Before: the patch held the lock for its
// whole build.) The f_i are big enough not to be inlined into `call`. Speculative compiles are off (MAKEPAD_RUN_SPEC=0) so each first call really compiles.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static STARTED: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(0);
static MAX_US: AtomicU64 = AtomicU64::new(0);

macro_rules! x4 {
    ($($t:tt)*) => { $($t)* $($t)* $($t)* $($t)* };
}

fn heavy(i: u64) -> u64 {
    let mut h = i;
    x4! { x4! { x4! { x4! { x4! { x4! { h = h.rotate_left(5) ^ (h >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15); } } } } } }
    h
}

fn f0(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(1);
        h = h.wrapping_add(0);
    }
    h
}
fn f1(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(3);
        h = h.wrapping_add(1);
    }
    h
}
fn f2(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(5);
        h = h.wrapping_add(2);
    }
    h
}
fn f3(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(7);
        h = h.wrapping_add(3);
    }
    h
}
fn f4(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(9);
        h = h.wrapping_add(4);
    }
    h
}
fn f5(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(11);
        h = h.wrapping_add(5);
    }
    h
}
fn f6(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(13);
        h = h.wrapping_add(6);
    }
    h
}
fn f7(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(15);
        h = h.wrapping_add(7);
    }
    h
}
fn f8(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(17);
        h = h.wrapping_add(8);
    }
    h
}
fn f9(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(19);
        h = h.wrapping_add(9);
    }
    h
}
fn f10(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(21);
        h = h.wrapping_add(10);
    }
    h
}
fn f11(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(23);
        h = h.wrapping_add(11);
    }
    h
}
fn f12(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(25);
        h = h.wrapping_add(12);
    }
    h
}
fn f13(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(27);
        h = h.wrapping_add(13);
    }
    h
}
fn f14(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(29);
        h = h.wrapping_add(14);
    }
    h
}
fn f15(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(31);
        h = h.wrapping_add(15);
    }
    h
}
fn f16(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(33);
        h = h.wrapping_add(16);
    }
    h
}
fn f17(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(35);
        h = h.wrapping_add(17);
    }
    h
}
fn f18(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(37);
        h = h.wrapping_add(18);
    }
    h
}
fn f19(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(39);
        h = h.wrapping_add(19);
    }
    h
}
fn f20(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(41);
        h = h.wrapping_add(20);
    }
    h
}
fn f21(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(43);
        h = h.wrapping_add(21);
    }
    h
}
fn f22(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(45);
        h = h.wrapping_add(22);
    }
    h
}
fn f23(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(47);
        h = h.wrapping_add(23);
    }
    h
}
fn f24(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(49);
        h = h.wrapping_add(24);
    }
    h
}
fn f25(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(51);
        h = h.wrapping_add(25);
    }
    h
}
fn f26(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(53);
        h = h.wrapping_add(26);
    }
    h
}
fn f27(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(55);
        h = h.wrapping_add(27);
    }
    h
}
fn f28(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(57);
        h = h.wrapping_add(28);
    }
    h
}
fn f29(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(59);
        h = h.wrapping_add(29);
    }
    h
}
fn f30(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(61);
        h = h.wrapping_add(30);
    }
    h
}
fn f31(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(63);
        h = h.wrapping_add(31);
    }
    h
}
fn f32(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(65);
        h = h.wrapping_add(32);
    }
    h
}
fn f33(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(67);
        h = h.wrapping_add(33);
    }
    h
}
fn f34(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(69);
        h = h.wrapping_add(34);
    }
    h
}
fn f35(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(71);
        h = h.wrapping_add(35);
    }
    h
}
fn f36(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(73);
        h = h.wrapping_add(36);
    }
    h
}
fn f37(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(75);
        h = h.wrapping_add(37);
    }
    h
}
fn f38(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(77);
        h = h.wrapping_add(38);
    }
    h
}
fn f39(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(79);
        h = h.wrapping_add(39);
    }
    h
}
fn f40(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(81);
        h = h.wrapping_add(40);
    }
    h
}
fn f41(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(83);
        h = h.wrapping_add(41);
    }
    h
}
fn f42(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(85);
        h = h.wrapping_add(42);
    }
    h
}
fn f43(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(87);
        h = h.wrapping_add(43);
    }
    h
}
fn f44(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(89);
        h = h.wrapping_add(44);
    }
    h
}
fn f45(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(91);
        h = h.wrapping_add(45);
    }
    h
}
fn f46(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(93);
        h = h.wrapping_add(46);
    }
    h
}
fn f47(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(95);
        h = h.wrapping_add(47);
    }
    h
}
fn f48(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(97);
        h = h.wrapping_add(48);
    }
    h
}
fn f49(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(99);
        h = h.wrapping_add(49);
    }
    h
}
fn f50(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(101);
        h = h.wrapping_add(50);
    }
    h
}
fn f51(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(103);
        h = h.wrapping_add(51);
    }
    h
}
fn f52(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(105);
        h = h.wrapping_add(52);
    }
    h
}
fn f53(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(107);
        h = h.wrapping_add(53);
    }
    h
}
fn f54(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(109);
        h = h.wrapping_add(54);
    }
    h
}
fn f55(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(111);
        h = h.wrapping_add(55);
    }
    h
}
fn f56(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(113);
        h = h.wrapping_add(56);
    }
    h
}
fn f57(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(115);
        h = h.wrapping_add(57);
    }
    h
}
fn f58(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(117);
        h = h.wrapping_add(58);
    }
    h
}
fn f59(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(119);
        h = h.wrapping_add(59);
    }
    h
}
fn f60(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(121);
        h = h.wrapping_add(60);
    }
    h
}
fn f61(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(123);
        h = h.wrapping_add(61);
    }
    h
}
fn f62(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(125);
        h = h.wrapping_add(62);
    }
    h
}
fn f63(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(127);
        h = h.wrapping_add(63);
    }
    h
}
fn f64(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(129);
        h = h.wrapping_add(64);
    }
    h
}
fn f65(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(131);
        h = h.wrapping_add(65);
    }
    h
}
fn f66(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(133);
        h = h.wrapping_add(66);
    }
    h
}
fn f67(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(135);
        h = h.wrapping_add(67);
    }
    h
}
fn f68(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(137);
        h = h.wrapping_add(68);
    }
    h
}
fn f69(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(139);
        h = h.wrapping_add(69);
    }
    h
}
fn f70(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(141);
        h = h.wrapping_add(70);
    }
    h
}
fn f71(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(143);
        h = h.wrapping_add(71);
    }
    h
}
fn f72(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(145);
        h = h.wrapping_add(72);
    }
    h
}
fn f73(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(147);
        h = h.wrapping_add(73);
    }
    h
}
fn f74(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(149);
        h = h.wrapping_add(74);
    }
    h
}
fn f75(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(151);
        h = h.wrapping_add(75);
    }
    h
}
fn f76(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(153);
        h = h.wrapping_add(76);
    }
    h
}
fn f77(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(155);
        h = h.wrapping_add(77);
    }
    h
}
fn f78(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(157);
        h = h.wrapping_add(78);
    }
    h
}
fn f79(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(159);
        h = h.wrapping_add(79);
    }
    h
}
fn f80(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(161);
        h = h.wrapping_add(80);
    }
    h
}
fn f81(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(163);
        h = h.wrapping_add(81);
    }
    h
}
fn f82(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(165);
        h = h.wrapping_add(82);
    }
    h
}
fn f83(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(167);
        h = h.wrapping_add(83);
    }
    h
}
fn f84(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(169);
        h = h.wrapping_add(84);
    }
    h
}
fn f85(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(171);
        h = h.wrapping_add(85);
    }
    h
}
fn f86(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(173);
        h = h.wrapping_add(86);
    }
    h
}
fn f87(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(175);
        h = h.wrapping_add(87);
    }
    h
}
fn f88(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(177);
        h = h.wrapping_add(88);
    }
    h
}
fn f89(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(179);
        h = h.wrapping_add(89);
    }
    h
}
fn f90(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(181);
        h = h.wrapping_add(90);
    }
    h
}
fn f91(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(183);
        h = h.wrapping_add(91);
    }
    h
}
fn f92(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(185);
        h = h.wrapping_add(92);
    }
    h
}
fn f93(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(187);
        h = h.wrapping_add(93);
    }
    h
}
fn f94(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(189);
        h = h.wrapping_add(94);
    }
    h
}
fn f95(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(191);
        h = h.wrapping_add(95);
    }
    h
}
fn f96(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(193);
        h = h.wrapping_add(96);
    }
    h
}
fn f97(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(195);
        h = h.wrapping_add(97);
    }
    h
}
fn f98(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(197);
        h = h.wrapping_add(98);
    }
    h
}
fn f99(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(199);
        h = h.wrapping_add(99);
    }
    h
}
fn f100(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(201);
        h = h.wrapping_add(100);
    }
    h
}
fn f101(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(203);
        h = h.wrapping_add(101);
    }
    h
}
fn f102(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(205);
        h = h.wrapping_add(102);
    }
    h
}
fn f103(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(207);
        h = h.wrapping_add(103);
    }
    h
}
fn f104(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(209);
        h = h.wrapping_add(104);
    }
    h
}
fn f105(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(211);
        h = h.wrapping_add(105);
    }
    h
}
fn f106(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(213);
        h = h.wrapping_add(106);
    }
    h
}
fn f107(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(215);
        h = h.wrapping_add(107);
    }
    h
}
fn f108(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(217);
        h = h.wrapping_add(108);
    }
    h
}
fn f109(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(219);
        h = h.wrapping_add(109);
    }
    h
}
fn f110(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(221);
        h = h.wrapping_add(110);
    }
    h
}
fn f111(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(223);
        h = h.wrapping_add(111);
    }
    h
}
fn f112(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(225);
        h = h.wrapping_add(112);
    }
    h
}
fn f113(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(227);
        h = h.wrapping_add(113);
    }
    h
}
fn f114(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(229);
        h = h.wrapping_add(114);
    }
    h
}
fn f115(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(231);
        h = h.wrapping_add(115);
    }
    h
}
fn f116(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(233);
        h = h.wrapping_add(116);
    }
    h
}
fn f117(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(235);
        h = h.wrapping_add(117);
    }
    h
}
fn f118(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(237);
        h = h.wrapping_add(118);
    }
    h
}
fn f119(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(239);
        h = h.wrapping_add(119);
    }
    h
}
fn f120(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(241);
        h = h.wrapping_add(120);
    }
    h
}
fn f121(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(243);
        h = h.wrapping_add(121);
    }
    h
}
fn f122(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(245);
        h = h.wrapping_add(122);
    }
    h
}
fn f123(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(247);
        h = h.wrapping_add(123);
    }
    h
}
fn f124(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(249);
        h = h.wrapping_add(124);
    }
    h
}
fn f125(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(251);
        h = h.wrapping_add(125);
    }
    h
}
fn f126(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(253);
        h = h.wrapping_add(126);
    }
    h
}
fn f127(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(255);
        h = h.wrapping_add(127);
    }
    h
}
fn f128(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(257);
        h = h.wrapping_add(128);
    }
    h
}
fn f129(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(259);
        h = h.wrapping_add(129);
    }
    h
}
fn f130(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(261);
        h = h.wrapping_add(130);
    }
    h
}
fn f131(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(263);
        h = h.wrapping_add(131);
    }
    h
}
fn f132(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(265);
        h = h.wrapping_add(132);
    }
    h
}
fn f133(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(267);
        h = h.wrapping_add(133);
    }
    h
}
fn f134(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(269);
        h = h.wrapping_add(134);
    }
    h
}
fn f135(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(271);
        h = h.wrapping_add(135);
    }
    h
}
fn f136(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(273);
        h = h.wrapping_add(136);
    }
    h
}
fn f137(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(275);
        h = h.wrapping_add(137);
    }
    h
}
fn f138(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(277);
        h = h.wrapping_add(138);
    }
    h
}
fn f139(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(279);
        h = h.wrapping_add(139);
    }
    h
}
fn f140(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(281);
        h = h.wrapping_add(140);
    }
    h
}
fn f141(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(283);
        h = h.wrapping_add(141);
    }
    h
}
fn f142(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(285);
        h = h.wrapping_add(142);
    }
    h
}
fn f143(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(287);
        h = h.wrapping_add(143);
    }
    h
}
fn f144(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(289);
        h = h.wrapping_add(144);
    }
    h
}
fn f145(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(291);
        h = h.wrapping_add(145);
    }
    h
}
fn f146(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(293);
        h = h.wrapping_add(146);
    }
    h
}
fn f147(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(295);
        h = h.wrapping_add(147);
    }
    h
}
fn f148(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(297);
        h = h.wrapping_add(148);
    }
    h
}
fn f149(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(299);
        h = h.wrapping_add(149);
    }
    h
}
fn f150(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(301);
        h = h.wrapping_add(150);
    }
    h
}
fn f151(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(303);
        h = h.wrapping_add(151);
    }
    h
}
fn f152(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(305);
        h = h.wrapping_add(152);
    }
    h
}
fn f153(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(307);
        h = h.wrapping_add(153);
    }
    h
}
fn f154(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(309);
        h = h.wrapping_add(154);
    }
    h
}
fn f155(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(311);
        h = h.wrapping_add(155);
    }
    h
}
fn f156(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(313);
        h = h.wrapping_add(156);
    }
    h
}
fn f157(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(315);
        h = h.wrapping_add(157);
    }
    h
}
fn f158(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(317);
        h = h.wrapping_add(158);
    }
    h
}
fn f159(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(319);
        h = h.wrapping_add(159);
    }
    h
}
fn f160(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(321);
        h = h.wrapping_add(160);
    }
    h
}
fn f161(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(323);
        h = h.wrapping_add(161);
    }
    h
}
fn f162(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(325);
        h = h.wrapping_add(162);
    }
    h
}
fn f163(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(327);
        h = h.wrapping_add(163);
    }
    h
}
fn f164(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(329);
        h = h.wrapping_add(164);
    }
    h
}
fn f165(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(331);
        h = h.wrapping_add(165);
    }
    h
}
fn f166(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(333);
        h = h.wrapping_add(166);
    }
    h
}
fn f167(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(335);
        h = h.wrapping_add(167);
    }
    h
}
fn f168(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(337);
        h = h.wrapping_add(168);
    }
    h
}
fn f169(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(339);
        h = h.wrapping_add(169);
    }
    h
}
fn f170(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(341);
        h = h.wrapping_add(170);
    }
    h
}
fn f171(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(343);
        h = h.wrapping_add(171);
    }
    h
}
fn f172(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(345);
        h = h.wrapping_add(172);
    }
    h
}
fn f173(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(347);
        h = h.wrapping_add(173);
    }
    h
}
fn f174(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(349);
        h = h.wrapping_add(174);
    }
    h
}
fn f175(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(351);
        h = h.wrapping_add(175);
    }
    h
}
fn f176(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(353);
        h = h.wrapping_add(176);
    }
    h
}
fn f177(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(355);
        h = h.wrapping_add(177);
    }
    h
}
fn f178(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(357);
        h = h.wrapping_add(178);
    }
    h
}
fn f179(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(359);
        h = h.wrapping_add(179);
    }
    h
}
fn f180(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(361);
        h = h.wrapping_add(180);
    }
    h
}
fn f181(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(363);
        h = h.wrapping_add(181);
    }
    h
}
fn f182(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(365);
        h = h.wrapping_add(182);
    }
    h
}
fn f183(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(367);
        h = h.wrapping_add(183);
    }
    h
}
fn f184(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(369);
        h = h.wrapping_add(184);
    }
    h
}
fn f185(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(371);
        h = h.wrapping_add(185);
    }
    h
}
fn f186(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(373);
        h = h.wrapping_add(186);
    }
    h
}
fn f187(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(375);
        h = h.wrapping_add(187);
    }
    h
}
fn f188(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(377);
        h = h.wrapping_add(188);
    }
    h
}
fn f189(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(379);
        h = h.wrapping_add(189);
    }
    h
}
fn f190(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(381);
        h = h.wrapping_add(190);
    }
    h
}
fn f191(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(383);
        h = h.wrapping_add(191);
    }
    h
}
fn f192(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(385);
        h = h.wrapping_add(192);
    }
    h
}
fn f193(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(387);
        h = h.wrapping_add(193);
    }
    h
}
fn f194(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(389);
        h = h.wrapping_add(194);
    }
    h
}
fn f195(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(391);
        h = h.wrapping_add(195);
    }
    h
}
fn f196(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(393);
        h = h.wrapping_add(196);
    }
    h
}
fn f197(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(395);
        h = h.wrapping_add(197);
    }
    h
}
fn f198(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(397);
        h = h.wrapping_add(198);
    }
    h
}
fn f199(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(399);
        h = h.wrapping_add(199);
    }
    h
}
fn f200(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(401);
        h = h.wrapping_add(200);
    }
    h
}
fn f201(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(403);
        h = h.wrapping_add(201);
    }
    h
}
fn f202(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(405);
        h = h.wrapping_add(202);
    }
    h
}
fn f203(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(407);
        h = h.wrapping_add(203);
    }
    h
}
fn f204(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(409);
        h = h.wrapping_add(204);
    }
    h
}
fn f205(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(411);
        h = h.wrapping_add(205);
    }
    h
}
fn f206(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(413);
        h = h.wrapping_add(206);
    }
    h
}
fn f207(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(415);
        h = h.wrapping_add(207);
    }
    h
}
fn f208(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(417);
        h = h.wrapping_add(208);
    }
    h
}
fn f209(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(419);
        h = h.wrapping_add(209);
    }
    h
}
fn f210(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(421);
        h = h.wrapping_add(210);
    }
    h
}
fn f211(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(423);
        h = h.wrapping_add(211);
    }
    h
}
fn f212(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(425);
        h = h.wrapping_add(212);
    }
    h
}
fn f213(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(427);
        h = h.wrapping_add(213);
    }
    h
}
fn f214(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(429);
        h = h.wrapping_add(214);
    }
    h
}
fn f215(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(431);
        h = h.wrapping_add(215);
    }
    h
}
fn f216(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(433);
        h = h.wrapping_add(216);
    }
    h
}
fn f217(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(435);
        h = h.wrapping_add(217);
    }
    h
}
fn f218(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(437);
        h = h.wrapping_add(218);
    }
    h
}
fn f219(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(439);
        h = h.wrapping_add(219);
    }
    h
}
fn f220(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(441);
        h = h.wrapping_add(220);
    }
    h
}
fn f221(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(443);
        h = h.wrapping_add(221);
    }
    h
}
fn f222(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(445);
        h = h.wrapping_add(222);
    }
    h
}
fn f223(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(447);
        h = h.wrapping_add(223);
    }
    h
}
fn f224(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(449);
        h = h.wrapping_add(224);
    }
    h
}
fn f225(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(451);
        h = h.wrapping_add(225);
    }
    h
}
fn f226(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(453);
        h = h.wrapping_add(226);
    }
    h
}
fn f227(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(455);
        h = h.wrapping_add(227);
    }
    h
}
fn f228(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(457);
        h = h.wrapping_add(228);
    }
    h
}
fn f229(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(459);
        h = h.wrapping_add(229);
    }
    h
}
fn f230(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(461);
        h = h.wrapping_add(230);
    }
    h
}
fn f231(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(463);
        h = h.wrapping_add(231);
    }
    h
}
fn f232(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(465);
        h = h.wrapping_add(232);
    }
    h
}
fn f233(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(467);
        h = h.wrapping_add(233);
    }
    h
}
fn f234(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(469);
        h = h.wrapping_add(234);
    }
    h
}
fn f235(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(471);
        h = h.wrapping_add(235);
    }
    h
}
fn f236(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(473);
        h = h.wrapping_add(236);
    }
    h
}
fn f237(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(475);
        h = h.wrapping_add(237);
    }
    h
}
fn f238(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(477);
        h = h.wrapping_add(238);
    }
    h
}
fn f239(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(479);
        h = h.wrapping_add(239);
    }
    h
}
fn f240(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(481);
        h = h.wrapping_add(240);
    }
    h
}
fn f241(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(483);
        h = h.wrapping_add(241);
    }
    h
}
fn f242(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(485);
        h = h.wrapping_add(242);
    }
    h
}
fn f243(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(487);
        h = h.wrapping_add(243);
    }
    h
}
fn f244(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(489);
        h = h.wrapping_add(244);
    }
    h
}
fn f245(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(491);
        h = h.wrapping_add(245);
    }
    h
}
fn f246(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(493);
        h = h.wrapping_add(246);
    }
    h
}
fn f247(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(495);
        h = h.wrapping_add(247);
    }
    h
}
fn f248(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(497);
        h = h.wrapping_add(248);
    }
    h
}
fn f249(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(499);
        h = h.wrapping_add(249);
    }
    h
}
fn f250(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(501);
        h = h.wrapping_add(250);
    }
    h
}
fn f251(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(503);
        h = h.wrapping_add(251);
    }
    h
}
fn f252(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(505);
        h = h.wrapping_add(252);
    }
    h
}
fn f253(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(507);
        h = h.wrapping_add(253);
    }
    h
}
fn f254(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(509);
        h = h.wrapping_add(254);
    }
    h
}
fn f255(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(511);
        h = h.wrapping_add(255);
    }
    h
}
fn f256(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(513);
        h = h.wrapping_add(256);
    }
    h
}
fn f257(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(515);
        h = h.wrapping_add(257);
    }
    h
}
fn f258(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(517);
        h = h.wrapping_add(258);
    }
    h
}
fn f259(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(519);
        h = h.wrapping_add(259);
    }
    h
}
fn f260(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(521);
        h = h.wrapping_add(260);
    }
    h
}
fn f261(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(523);
        h = h.wrapping_add(261);
    }
    h
}
fn f262(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(525);
        h = h.wrapping_add(262);
    }
    h
}
fn f263(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(527);
        h = h.wrapping_add(263);
    }
    h
}
fn f264(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(529);
        h = h.wrapping_add(264);
    }
    h
}
fn f265(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(531);
        h = h.wrapping_add(265);
    }
    h
}
fn f266(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(533);
        h = h.wrapping_add(266);
    }
    h
}
fn f267(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(535);
        h = h.wrapping_add(267);
    }
    h
}
fn f268(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(537);
        h = h.wrapping_add(268);
    }
    h
}
fn f269(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(539);
        h = h.wrapping_add(269);
    }
    h
}
fn f270(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(541);
        h = h.wrapping_add(270);
    }
    h
}
fn f271(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(543);
        h = h.wrapping_add(271);
    }
    h
}
fn f272(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(545);
        h = h.wrapping_add(272);
    }
    h
}
fn f273(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(547);
        h = h.wrapping_add(273);
    }
    h
}
fn f274(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(549);
        h = h.wrapping_add(274);
    }
    h
}
fn f275(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(551);
        h = h.wrapping_add(275);
    }
    h
}
fn f276(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(553);
        h = h.wrapping_add(276);
    }
    h
}
fn f277(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(555);
        h = h.wrapping_add(277);
    }
    h
}
fn f278(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(557);
        h = h.wrapping_add(278);
    }
    h
}
fn f279(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(559);
        h = h.wrapping_add(279);
    }
    h
}
fn f280(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(561);
        h = h.wrapping_add(280);
    }
    h
}
fn f281(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(563);
        h = h.wrapping_add(281);
    }
    h
}
fn f282(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(565);
        h = h.wrapping_add(282);
    }
    h
}
fn f283(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(567);
        h = h.wrapping_add(283);
    }
    h
}
fn f284(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(569);
        h = h.wrapping_add(284);
    }
    h
}
fn f285(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(571);
        h = h.wrapping_add(285);
    }
    h
}
fn f286(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(573);
        h = h.wrapping_add(286);
    }
    h
}
fn f287(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(575);
        h = h.wrapping_add(287);
    }
    h
}
fn f288(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(577);
        h = h.wrapping_add(288);
    }
    h
}
fn f289(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(579);
        h = h.wrapping_add(289);
    }
    h
}
fn f290(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(581);
        h = h.wrapping_add(290);
    }
    h
}
fn f291(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(583);
        h = h.wrapping_add(291);
    }
    h
}
fn f292(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(585);
        h = h.wrapping_add(292);
    }
    h
}
fn f293(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(587);
        h = h.wrapping_add(293);
    }
    h
}
fn f294(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(589);
        h = h.wrapping_add(294);
    }
    h
}
fn f295(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(591);
        h = h.wrapping_add(295);
    }
    h
}
fn f296(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(593);
        h = h.wrapping_add(296);
    }
    h
}
fn f297(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(595);
        h = h.wrapping_add(297);
    }
    h
}
fn f298(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(597);
        h = h.wrapping_add(298);
    }
    h
}
fn f299(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(599);
        h = h.wrapping_add(299);
    }
    h
}
fn f300(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(601);
        h = h.wrapping_add(300);
    }
    h
}
fn f301(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(603);
        h = h.wrapping_add(301);
    }
    h
}
fn f302(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(605);
        h = h.wrapping_add(302);
    }
    h
}
fn f303(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(607);
        h = h.wrapping_add(303);
    }
    h
}
fn f304(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(609);
        h = h.wrapping_add(304);
    }
    h
}
fn f305(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(611);
        h = h.wrapping_add(305);
    }
    h
}
fn f306(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(613);
        h = h.wrapping_add(306);
    }
    h
}
fn f307(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(615);
        h = h.wrapping_add(307);
    }
    h
}
fn f308(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(617);
        h = h.wrapping_add(308);
    }
    h
}
fn f309(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(619);
        h = h.wrapping_add(309);
    }
    h
}
fn f310(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(621);
        h = h.wrapping_add(310);
    }
    h
}
fn f311(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(623);
        h = h.wrapping_add(311);
    }
    h
}
fn f312(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(625);
        h = h.wrapping_add(312);
    }
    h
}
fn f313(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(627);
        h = h.wrapping_add(313);
    }
    h
}
fn f314(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(629);
        h = h.wrapping_add(314);
    }
    h
}
fn f315(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(631);
        h = h.wrapping_add(315);
    }
    h
}
fn f316(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(633);
        h = h.wrapping_add(316);
    }
    h
}
fn f317(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(635);
        h = h.wrapping_add(317);
    }
    h
}
fn f318(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(637);
        h = h.wrapping_add(318);
    }
    h
}
fn f319(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(639);
        h = h.wrapping_add(319);
    }
    h
}
fn f320(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(641);
        h = h.wrapping_add(320);
    }
    h
}
fn f321(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(643);
        h = h.wrapping_add(321);
    }
    h
}
fn f322(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(645);
        h = h.wrapping_add(322);
    }
    h
}
fn f323(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(647);
        h = h.wrapping_add(323);
    }
    h
}
fn f324(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(649);
        h = h.wrapping_add(324);
    }
    h
}
fn f325(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(651);
        h = h.wrapping_add(325);
    }
    h
}
fn f326(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(653);
        h = h.wrapping_add(326);
    }
    h
}
fn f327(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(655);
        h = h.wrapping_add(327);
    }
    h
}
fn f328(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(657);
        h = h.wrapping_add(328);
    }
    h
}
fn f329(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(659);
        h = h.wrapping_add(329);
    }
    h
}
fn f330(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(661);
        h = h.wrapping_add(330);
    }
    h
}
fn f331(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(663);
        h = h.wrapping_add(331);
    }
    h
}
fn f332(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(665);
        h = h.wrapping_add(332);
    }
    h
}
fn f333(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(667);
        h = h.wrapping_add(333);
    }
    h
}
fn f334(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(669);
        h = h.wrapping_add(334);
    }
    h
}
fn f335(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(671);
        h = h.wrapping_add(335);
    }
    h
}
fn f336(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(673);
        h = h.wrapping_add(336);
    }
    h
}
fn f337(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(675);
        h = h.wrapping_add(337);
    }
    h
}
fn f338(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(677);
        h = h.wrapping_add(338);
    }
    h
}
fn f339(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(679);
        h = h.wrapping_add(339);
    }
    h
}
fn f340(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(681);
        h = h.wrapping_add(340);
    }
    h
}
fn f341(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(683);
        h = h.wrapping_add(341);
    }
    h
}
fn f342(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(685);
        h = h.wrapping_add(342);
    }
    h
}
fn f343(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(687);
        h = h.wrapping_add(343);
    }
    h
}
fn f344(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(689);
        h = h.wrapping_add(344);
    }
    h
}
fn f345(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(691);
        h = h.wrapping_add(345);
    }
    h
}
fn f346(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(693);
        h = h.wrapping_add(346);
    }
    h
}
fn f347(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(695);
        h = h.wrapping_add(347);
    }
    h
}
fn f348(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(697);
        h = h.wrapping_add(348);
    }
    h
}
fn f349(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(699);
        h = h.wrapping_add(349);
    }
    h
}
fn f350(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(701);
        h = h.wrapping_add(350);
    }
    h
}
fn f351(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(703);
        h = h.wrapping_add(351);
    }
    h
}
fn f352(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(705);
        h = h.wrapping_add(352);
    }
    h
}
fn f353(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(707);
        h = h.wrapping_add(353);
    }
    h
}
fn f354(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(709);
        h = h.wrapping_add(354);
    }
    h
}
fn f355(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(711);
        h = h.wrapping_add(355);
    }
    h
}
fn f356(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(713);
        h = h.wrapping_add(356);
    }
    h
}
fn f357(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(715);
        h = h.wrapping_add(357);
    }
    h
}
fn f358(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(717);
        h = h.wrapping_add(358);
    }
    h
}
fn f359(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(719);
        h = h.wrapping_add(359);
    }
    h
}
fn f360(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(721);
        h = h.wrapping_add(360);
    }
    h
}
fn f361(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(723);
        h = h.wrapping_add(361);
    }
    h
}
fn f362(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(725);
        h = h.wrapping_add(362);
    }
    h
}
fn f363(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(727);
        h = h.wrapping_add(363);
    }
    h
}
fn f364(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(729);
        h = h.wrapping_add(364);
    }
    h
}
fn f365(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(731);
        h = h.wrapping_add(365);
    }
    h
}
fn f366(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(733);
        h = h.wrapping_add(366);
    }
    h
}
fn f367(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(735);
        h = h.wrapping_add(367);
    }
    h
}
fn f368(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(737);
        h = h.wrapping_add(368);
    }
    h
}
fn f369(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(739);
        h = h.wrapping_add(369);
    }
    h
}
fn f370(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(741);
        h = h.wrapping_add(370);
    }
    h
}
fn f371(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(743);
        h = h.wrapping_add(371);
    }
    h
}
fn f372(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(745);
        h = h.wrapping_add(372);
    }
    h
}
fn f373(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(747);
        h = h.wrapping_add(373);
    }
    h
}
fn f374(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(749);
        h = h.wrapping_add(374);
    }
    h
}
fn f375(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(751);
        h = h.wrapping_add(375);
    }
    h
}
fn f376(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(753);
        h = h.wrapping_add(376);
    }
    h
}
fn f377(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(755);
        h = h.wrapping_add(377);
    }
    h
}
fn f378(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(757);
        h = h.wrapping_add(378);
    }
    h
}
fn f379(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(759);
        h = h.wrapping_add(379);
    }
    h
}
fn f380(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(761);
        h = h.wrapping_add(380);
    }
    h
}
fn f381(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(763);
        h = h.wrapping_add(381);
    }
    h
}
fn f382(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(765);
        h = h.wrapping_add(382);
    }
    h
}
fn f383(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(767);
        h = h.wrapping_add(383);
    }
    h
}
fn f384(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(769);
        h = h.wrapping_add(384);
    }
    h
}
fn f385(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(771);
        h = h.wrapping_add(385);
    }
    h
}
fn f386(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(773);
        h = h.wrapping_add(386);
    }
    h
}
fn f387(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(775);
        h = h.wrapping_add(387);
    }
    h
}
fn f388(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(777);
        h = h.wrapping_add(388);
    }
    h
}
fn f389(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(779);
        h = h.wrapping_add(389);
    }
    h
}
fn f390(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(781);
        h = h.wrapping_add(390);
    }
    h
}
fn f391(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(783);
        h = h.wrapping_add(391);
    }
    h
}
fn f392(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(785);
        h = h.wrapping_add(392);
    }
    h
}
fn f393(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(787);
        h = h.wrapping_add(393);
    }
    h
}
fn f394(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(789);
        h = h.wrapping_add(394);
    }
    h
}
fn f395(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(791);
        h = h.wrapping_add(395);
    }
    h
}
fn f396(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(793);
        h = h.wrapping_add(396);
    }
    h
}
fn f397(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(795);
        h = h.wrapping_add(397);
    }
    h
}
fn f398(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(797);
        h = h.wrapping_add(398);
    }
    h
}
fn f399(x: u64) -> u64 {
    let mut h = x;
    for i in 0..3u64 {
        h = h.rotate_left(7) ^ i.wrapping_mul(799);
        h = h.wrapping_add(399);
    }
    h
}

fn call(k: u64, x: u64) -> u64 {
    match k {
        0 => f0(x),
        1 => f1(x),
        2 => f2(x),
        3 => f3(x),
        4 => f4(x),
        5 => f5(x),
        6 => f6(x),
        7 => f7(x),
        8 => f8(x),
        9 => f9(x),
        10 => f10(x),
        11 => f11(x),
        12 => f12(x),
        13 => f13(x),
        14 => f14(x),
        15 => f15(x),
        16 => f16(x),
        17 => f17(x),
        18 => f18(x),
        19 => f19(x),
        20 => f20(x),
        21 => f21(x),
        22 => f22(x),
        23 => f23(x),
        24 => f24(x),
        25 => f25(x),
        26 => f26(x),
        27 => f27(x),
        28 => f28(x),
        29 => f29(x),
        30 => f30(x),
        31 => f31(x),
        32 => f32(x),
        33 => f33(x),
        34 => f34(x),
        35 => f35(x),
        36 => f36(x),
        37 => f37(x),
        38 => f38(x),
        39 => f39(x),
        40 => f40(x),
        41 => f41(x),
        42 => f42(x),
        43 => f43(x),
        44 => f44(x),
        45 => f45(x),
        46 => f46(x),
        47 => f47(x),
        48 => f48(x),
        49 => f49(x),
        50 => f50(x),
        51 => f51(x),
        52 => f52(x),
        53 => f53(x),
        54 => f54(x),
        55 => f55(x),
        56 => f56(x),
        57 => f57(x),
        58 => f58(x),
        59 => f59(x),
        60 => f60(x),
        61 => f61(x),
        62 => f62(x),
        63 => f63(x),
        64 => f64(x),
        65 => f65(x),
        66 => f66(x),
        67 => f67(x),
        68 => f68(x),
        69 => f69(x),
        70 => f70(x),
        71 => f71(x),
        72 => f72(x),
        73 => f73(x),
        74 => f74(x),
        75 => f75(x),
        76 => f76(x),
        77 => f77(x),
        78 => f78(x),
        79 => f79(x),
        80 => f80(x),
        81 => f81(x),
        82 => f82(x),
        83 => f83(x),
        84 => f84(x),
        85 => f85(x),
        86 => f86(x),
        87 => f87(x),
        88 => f88(x),
        89 => f89(x),
        90 => f90(x),
        91 => f91(x),
        92 => f92(x),
        93 => f93(x),
        94 => f94(x),
        95 => f95(x),
        96 => f96(x),
        97 => f97(x),
        98 => f98(x),
        99 => f99(x),
        100 => f100(x),
        101 => f101(x),
        102 => f102(x),
        103 => f103(x),
        104 => f104(x),
        105 => f105(x),
        106 => f106(x),
        107 => f107(x),
        108 => f108(x),
        109 => f109(x),
        110 => f110(x),
        111 => f111(x),
        112 => f112(x),
        113 => f113(x),
        114 => f114(x),
        115 => f115(x),
        116 => f116(x),
        117 => f117(x),
        118 => f118(x),
        119 => f119(x),
        120 => f120(x),
        121 => f121(x),
        122 => f122(x),
        123 => f123(x),
        124 => f124(x),
        125 => f125(x),
        126 => f126(x),
        127 => f127(x),
        128 => f128(x),
        129 => f129(x),
        130 => f130(x),
        131 => f131(x),
        132 => f132(x),
        133 => f133(x),
        134 => f134(x),
        135 => f135(x),
        136 => f136(x),
        137 => f137(x),
        138 => f138(x),
        139 => f139(x),
        140 => f140(x),
        141 => f141(x),
        142 => f142(x),
        143 => f143(x),
        144 => f144(x),
        145 => f145(x),
        146 => f146(x),
        147 => f147(x),
        148 => f148(x),
        149 => f149(x),
        150 => f150(x),
        151 => f151(x),
        152 => f152(x),
        153 => f153(x),
        154 => f154(x),
        155 => f155(x),
        156 => f156(x),
        157 => f157(x),
        158 => f158(x),
        159 => f159(x),
        160 => f160(x),
        161 => f161(x),
        162 => f162(x),
        163 => f163(x),
        164 => f164(x),
        165 => f165(x),
        166 => f166(x),
        167 => f167(x),
        168 => f168(x),
        169 => f169(x),
        170 => f170(x),
        171 => f171(x),
        172 => f172(x),
        173 => f173(x),
        174 => f174(x),
        175 => f175(x),
        176 => f176(x),
        177 => f177(x),
        178 => f178(x),
        179 => f179(x),
        180 => f180(x),
        181 => f181(x),
        182 => f182(x),
        183 => f183(x),
        184 => f184(x),
        185 => f185(x),
        186 => f186(x),
        187 => f187(x),
        188 => f188(x),
        189 => f189(x),
        190 => f190(x),
        191 => f191(x),
        192 => f192(x),
        193 => f193(x),
        194 => f194(x),
        195 => f195(x),
        196 => f196(x),
        197 => f197(x),
        198 => f198(x),
        199 => f199(x),
        200 => f200(x),
        201 => f201(x),
        202 => f202(x),
        203 => f203(x),
        204 => f204(x),
        205 => f205(x),
        206 => f206(x),
        207 => f207(x),
        208 => f208(x),
        209 => f209(x),
        210 => f210(x),
        211 => f211(x),
        212 => f212(x),
        213 => f213(x),
        214 => f214(x),
        215 => f215(x),
        216 => f216(x),
        217 => f217(x),
        218 => f218(x),
        219 => f219(x),
        220 => f220(x),
        221 => f221(x),
        222 => f222(x),
        223 => f223(x),
        224 => f224(x),
        225 => f225(x),
        226 => f226(x),
        227 => f227(x),
        228 => f228(x),
        229 => f229(x),
        230 => f230(x),
        231 => f231(x),
        232 => f232(x),
        233 => f233(x),
        234 => f234(x),
        235 => f235(x),
        236 => f236(x),
        237 => f237(x),
        238 => f238(x),
        239 => f239(x),
        240 => f240(x),
        241 => f241(x),
        242 => f242(x),
        243 => f243(x),
        244 => f244(x),
        245 => f245(x),
        246 => f246(x),
        247 => f247(x),
        248 => f248(x),
        249 => f249(x),
        250 => f250(x),
        251 => f251(x),
        252 => f252(x),
        253 => f253(x),
        254 => f254(x),
        255 => f255(x),
        256 => f256(x),
        257 => f257(x),
        258 => f258(x),
        259 => f259(x),
        260 => f260(x),
        261 => f261(x),
        262 => f262(x),
        263 => f263(x),
        264 => f264(x),
        265 => f265(x),
        266 => f266(x),
        267 => f267(x),
        268 => f268(x),
        269 => f269(x),
        270 => f270(x),
        271 => f271(x),
        272 => f272(x),
        273 => f273(x),
        274 => f274(x),
        275 => f275(x),
        276 => f276(x),
        277 => f277(x),
        278 => f278(x),
        279 => f279(x),
        280 => f280(x),
        281 => f281(x),
        282 => f282(x),
        283 => f283(x),
        284 => f284(x),
        285 => f285(x),
        286 => f286(x),
        287 => f287(x),
        288 => f288(x),
        289 => f289(x),
        290 => f290(x),
        291 => f291(x),
        292 => f292(x),
        293 => f293(x),
        294 => f294(x),
        295 => f295(x),
        296 => f296(x),
        297 => f297(x),
        298 => f298(x),
        299 => f299(x),
        300 => f300(x),
        301 => f301(x),
        302 => f302(x),
        303 => f303(x),
        304 => f304(x),
        305 => f305(x),
        306 => f306(x),
        307 => f307(x),
        308 => f308(x),
        309 => f309(x),
        310 => f310(x),
        311 => f311(x),
        312 => f312(x),
        313 => f313(x),
        314 => f314(x),
        315 => f315(x),
        316 => f316(x),
        317 => f317(x),
        318 => f318(x),
        319 => f319(x),
        320 => f320(x),
        321 => f321(x),
        322 => f322(x),
        323 => f323(x),
        324 => f324(x),
        325 => f325(x),
        326 => f326(x),
        327 => f327(x),
        328 => f328(x),
        329 => f329(x),
        330 => f330(x),
        331 => f331(x),
        332 => f332(x),
        333 => f333(x),
        334 => f334(x),
        335 => f335(x),
        336 => f336(x),
        337 => f337(x),
        338 => f338(x),
        339 => f339(x),
        340 => f340(x),
        341 => f341(x),
        342 => f342(x),
        343 => f343(x),
        344 => f344(x),
        345 => f345(x),
        346 => f346(x),
        347 => f347(x),
        348 => f348(x),
        349 => f349(x),
        350 => f350(x),
        351 => f351(x),
        352 => f352(x),
        353 => f353(x),
        354 => f354(x),
        355 => f355(x),
        356 => f356(x),
        357 => f357(x),
        358 => f358(x),
        359 => f359(x),
        360 => f360(x),
        361 => f361(x),
        362 => f362(x),
        363 => f363(x),
        364 => f364(x),
        365 => f365(x),
        366 => f366(x),
        367 => f367(x),
        368 => f368(x),
        369 => f369(x),
        370 => f370(x),
        371 => f371(x),
        372 => f372(x),
        373 => f373(x),
        374 => f374(x),
        375 => f375(x),
        376 => f376(x),
        377 => f377(x),
        378 => f378(x),
        379 => f379(x),
        380 => f380(x),
        381 => f381(x),
        382 => f382(x),
        383 => f383(x),
        384 => f384(x),
        385 => f385(x),
        386 => f386(x),
        387 => f387(x),
        388 => f388(x),
        389 => f389(x),
        390 => f390(x),
        391 => f391(x),
        392 => f392(x),
        393 => f393(x),
        394 => f394(x),
        395 => f395(x),
        396 => f396(x),
        397 => f397(x),
        398 => f398(x),
        399 => f399(x),
        _ => x,
    }
}

fn worker() {
    loop {
        let k = NEXT.fetch_add(1, Ordering::SeqCst);
        if k >= 400 {
            break;
        }
        let t = Instant::now();
        let _ = call(k, 7);
        MAX_US.fetch_max(t.elapsed().as_micros() as u64, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn entry(x: u64) -> u64 {
    heavy(x) & 1
}

#[test]
fn run() {
    if !STARTED.swap(true, Ordering::SeqCst) {
        println!("out first {}", entry(1));
        std::thread::spawn(worker);
        // the first lazy compiles of the worker are its own warm-up
        while NEXT.load(Ordering::SeqCst) < 20 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        MAX_US.store(0, Ordering::SeqCst);
        return;
    }
    let m = MAX_US.swap(0, Ordering::SeqCst);
    println!("out entry {}", entry(1));
    if m < 100_000 {
        println!("out waits ok");
    } else {
        println!("out waited {} ms", m / 1000);
    }
}
