//! Common-subexpression elimination merges two instructions only when their results have the
//! same class: `VExtract` of one lane into an I vreg (sign-extended i32) and into an F32 vreg have
//! the same key but are two values. A Splash x4 kernel stored the integer register as an f32 when
//! the F32 extract was replaced by a copy of the I one.
use relax_backend::passes;
use relax_backend::rir::*;

#[test]
fn extracts_of_one_lane_as_int_and_float_stay_apart() {
    let mut f = Func::new("cse_result_class".into(), 0);
    let (src, dst) = (f.vreg(Cls::I), f.vreg(Cls::I));
    f.params = vec![src, dst];
    f.block();
    let (v, xi, xf) = (f.vreg(Cls::V128), f.vreg(Cls::I), f.vreg(Cls::F32));
    let insts = vec![
        Inst::VLoad(v, src, 0),
        Inst::VExtract(xi, v, 2),
        Inst::VExtract(xf, v, 2),
        Inst::Store(Mem::Int(4, false), dst, 0, xi),
        Inst::Store(Mem::F32, dst, 4, xf),
    ];
    f.blocks[0].pos = vec![0; insts.len()];
    f.blocks[0].insts = insts;
    f.blocks[0].term = Term::Ret(vec![]);
    f.verify().unwrap();
    passes::simplify(&relax_session::Options::default(), &mut f, None, true, true);
    passes::local_cse(&relax_session::Options::default(), &mut f);
    f.verify().unwrap();
    // every store still stores a value of its own memory class
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Store(m, _, _, val) = i {
                let cls = f.vregs[val.0 as usize];
                match m {
                    Mem::F32 => assert_eq!(cls, Cls::F32, "the f32 store reads a {cls:?} vreg"),
                    Mem::Int(..) => assert_eq!(cls, Cls::I, "the i32 store reads a {cls:?} vreg"),
                    _ => {}
                }
            }
            if let Inst::Mov(d, s) = i {
                assert_eq!(f.vregs[d.0 as usize], f.vregs[s.0 as usize], "a copy across classes");
            }
        }
    }
}
