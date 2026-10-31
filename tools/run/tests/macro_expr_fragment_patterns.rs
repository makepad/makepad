// Regression: a `$f:expr` fragment forwarded through nested macro_rules (macro-generated functions
// whose bodies hold tuple patterns) must not trip "arbitrary expressions aren't allowed in patterns"
// (rustc_ast_passes check_expr_within_pat). The transcription's synthetic parens around the
// multi-token fragment were recorded in a process-global set keyed by position, so later expansions
// found their own real `let (x0, ip) = ..` patterns in it (the 17th invocation failed; libs/stitch/src/exec.rs).
// run: makepad check <package> with this file as its lib.rs, or makepad test tools/run/tests/macro_expr_fragment_patterns.rs

type Ip=u32;type Sp=u32;type Md=u32;type Ms=u32;type Ix=u32;type Sx=u32;type Dx=u32;type Cx=u32;type ControlFlowBits=u32;
enum ControlFlow{Trap(u32)} impl ControlFlow{fn to_bits(self)->u32{0}}
fn read_stack(a:u32,b:u32)->(u32,u32){(a,b)} fn read_imm(a:u32)->(u32,u32){(a,a)} fn read_reg(a:u32,b:u32,c:u32)->u32{a+b+c}
fn write_reg(a:u32,b:u32,c:u32,y:u32)->(u32,u32,u32){(a,b,y+c)}
fn next_instr(_a:u32,_b:u32,_c:u32,_d:u32,_e:u32,_f:u32,_g:u32,_h:u32)->u32{0}
trait RelOps{fn gt(a:Self,b:Self)->Result<u32,u32> where Self:Sized;}
impl RelOps for u32{fn gt(a:u32,b:u32)->Result<u32,u32>{Ok((a>b) as u32)}}
macro_rules! threaded_instr {
    ($name:ident(
        $ip:ident: Ip,
        $sp:ident: Sp,
        $md:ident: Md,
        $ms:ident: Ms,
        $ix:ident: Ix,
        $sx:ident: Sx,
        $dx:ident: Dx,
        $cx:ident: Cx,
    ) -> ControlFlowBits $body:block) => {
        pub(crate) unsafe extern "C" fn $name(
            $ip: Ip,
            $sp: Sp,
            $md: Md,
            $ms: Ms,
            $ix: Ix,
            $sx: Sx,
            $dx: Dx,
            $cx: Cx,
        ) -> ControlFlowBits $body
    };
}
macro_rules! r#try {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(trap) => return ControlFlow::Trap(trap).to_bits(),
        }
    };
}
macro_rules! bin_op {
    (
        $bin_op_ss:ident,
        $bin_op_rs:ident,
        $bin_op_is:ident,
        $bin_op_ir:ident,
        $f:expr
    ) => {
        threaded_instr!($bin_op_ss(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_stack(ip, sp);
            let (x0, ip) = read_stack(ip, sp);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });

        threaded_instr!($bin_op_rs(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_stack(ip, sp);
            let x0 = read_reg(ix, sx, dx);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });

        threaded_instr!($bin_op_is(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_stack(ip, sp);
            let (x0, ip) = read_imm(ip);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });

        threaded_instr!($bin_op_ir(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x1 = read_reg(ix, sx, dx);
            let (x0, ip) = read_imm(ip);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });
    };
}
macro_rules! bin_op_noncommutative {
    (
        $bin_op_ss:ident,
        $bin_op_rs:ident,
        $bin_op_is:ident,
        $bin_op_ir:ident,
        $bin_op_sr:ident,
        $bin_op_si:ident,
        $bin_op_ri:ident,
        $f:expr
    ) => {
        bin_op!($bin_op_ss, $bin_op_rs, $bin_op_is, $bin_op_ir, $f);

        threaded_instr!($bin_op_sr(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let x1 = read_reg(ix, sx, dx);
            let (x0, ip) = read_stack(ip, sp);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });

        threaded_instr!($bin_op_si(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_imm(ip);
            let (x0, ip) = read_stack(ip, sp);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });

        threaded_instr!($bin_op_ri(
            ip: Ip,
            sp: Sp,
            md: Md,
            ms: Ms,
            ix: Ix,
            sx: Sx,
            dx: Dx,
            cx: Cx,
        ) -> ControlFlowBits {
            // Read operands
            let (x1, ip) = read_imm(ip);
            let x0 = read_reg(ix, sx, dx);

            // Perform operation
            let y = r#try!($f(x0, x1));

            // Write result
            let (ix, sx, dx) = write_reg(ix, sx, dx, y);

            // Execute next instruction
            next_instr(ip, sp, md, ms, ix, sx, dx, cx)
        });
    };
}
bin_op_noncommutative!(a01,a02,a03,a04,a05,a06,a07,<u32 as RelOps>::gt);
bin_op_noncommutative!(a11,a12,a13,a14,a15,a16,a17,<u32 as RelOps>::gt);
bin_op_noncommutative!(a21,a22,a23,a24,a25,a26,a27,<u32 as RelOps>::gt);
bin_op_noncommutative!(a31,a32,a33,a34,a35,a36,a37,<u32 as RelOps>::gt);
bin_op_noncommutative!(a41,a42,a43,a44,a45,a46,a47,<u32 as RelOps>::gt);
bin_op_noncommutative!(a51,a52,a53,a54,a55,a56,a57,<u32 as RelOps>::gt);
bin_op_noncommutative!(a61,a62,a63,a64,a65,a66,a67,<u32 as RelOps>::gt);
bin_op_noncommutative!(a71,a72,a73,a74,a75,a76,a77,<u32 as RelOps>::gt);
bin_op_noncommutative!(a81,a82,a83,a84,a85,a86,a87,<u32 as RelOps>::gt);
bin_op_noncommutative!(a91,a92,a93,a94,a95,a96,a97,<u32 as RelOps>::gt);
bin_op_noncommutative!(a101,a102,a103,a104,a105,a106,a107,<u32 as RelOps>::gt);
bin_op_noncommutative!(a111,a112,a113,a114,a115,a116,a117,<u32 as RelOps>::gt);
bin_op_noncommutative!(a121,a122,a123,a124,a125,a126,a127,<u32 as RelOps>::gt);
bin_op_noncommutative!(a131,a132,a133,a134,a135,a136,a137,<u32 as RelOps>::gt);
bin_op_noncommutative!(a141,a142,a143,a144,a145,a146,a147,<u32 as RelOps>::gt);
bin_op_noncommutative!(a151,a152,a153,a154,a155,a156,a157,<u32 as RelOps>::gt);
bin_op_noncommutative!(a161,a162,a163,a164,a165,a166,a167,<u32 as RelOps>::gt);
bin_op_noncommutative!(a171,a172,a173,a174,a175,a176,a177,<u32 as RelOps>::gt);
bin_op_noncommutative!(a181,a182,a183,a184,a185,a186,a187,<u32 as RelOps>::gt);
bin_op_noncommutative!(a191,a192,a193,a194,a195,a196,a197,<u32 as RelOps>::gt);
bin_op_noncommutative!(a201,a202,a203,a204,a205,a206,a207,<u32 as RelOps>::gt);
bin_op_noncommutative!(a211,a212,a213,a214,a215,a216,a217,<u32 as RelOps>::gt);
bin_op_noncommutative!(a221,a222,a223,a224,a225,a226,a227,<u32 as RelOps>::gt);
bin_op_noncommutative!(a231,a232,a233,a234,a235,a236,a237,<u32 as RelOps>::gt);
bin_op_noncommutative!(a241,a242,a243,a244,a245,a246,a247,<u32 as RelOps>::gt);
bin_op_noncommutative!(a251,a252,a253,a254,a255,a256,a257,<u32 as RelOps>::gt);
bin_op_noncommutative!(a261,a262,a263,a264,a265,a266,a267,<u32 as RelOps>::gt);
bin_op_noncommutative!(a271,a272,a273,a274,a275,a276,a277,<u32 as RelOps>::gt);
bin_op_noncommutative!(a281,a282,a283,a284,a285,a286,a287,<u32 as RelOps>::gt);
bin_op_noncommutative!(a291,a292,a293,a294,a295,a296,a297,<u32 as RelOps>::gt);
