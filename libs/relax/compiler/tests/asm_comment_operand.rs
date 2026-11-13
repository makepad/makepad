// zeroize 1.9 barrier.rs optimization_barrier: `asm!("# {}", in(reg) ptr, ..)` substitutes the
// operand inside a line comment; the comment runs to the end of the line, operand included (LLVM
// parses the template after substitution), so the statement is empty. A `#` that starts a statement
// is a comment on every target (LLVM AsmLexer), aarch64 included (relax #91).
// needs-native: inline assembly
#[test]
fn operand_inside_line_comment() {
    let v = [1u8, 2, 3];
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    unsafe {
        core::arch::asm!("# {}", in(reg) v.as_ptr(), options(readonly, preserves_flags, nostack));
        core::arch::asm!("# {} and {}\n nop", in(reg) v.as_ptr(), in(reg) 7usize, options(readonly, preserves_flags, nostack));
        core::arch::asm!("nop\n  # {}", in(reg) v.as_ptr(), options(readonly, preserves_flags, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("// {}", in(reg) v.as_ptr(), options(readonly, preserves_flags, nostack));
        core::arch::asm!("// {} and {}\n nop", in(reg) v.as_ptr(), in(reg) 7usize, options(readonly, preserves_flags, nostack));
    }
    assert_eq!(v.iter().sum::<u8>(), 6);
}

#[test]
fn block_comment_around_operand() {
    let mut x = 5u64;
    #[cfg(target_arch = "x86_64")]
    unsafe { core::arch::asm!("/* {} */ add {0}, 1", inout(reg) x); }
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("/* {0} */ add {0}, {0}, #1", inout(reg) x); }
    assert_eq!(x, 6);
}
