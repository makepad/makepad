#!/usr/bin/env python3
# gen_check.py <table.txt> <out check.rs>: one const assertion per size, align and field offset of
# the reference table (wasm_ref.rs output), for `makepad check` under the web target (run.sh).
import sys

rows = [l.rstrip('\n') for l in open(sys.argv[1]) if l.strip()]
out = ['#![allow(dead_code, unused_macros)]', 'include!("corpus.rs");', 'fn main() {}']
for n, row in enumerate(rows):
    ty, size, align, offs = row.split('|')
    tag = ty.replace('\\', '\\\\').replace('"', '\\"')
    out.append(f'const _: () = assert!(std::mem::size_of::<{ty}>() == {size}, "row {n}: size of {tag} is not {size}");')
    out.append(f'const _: () = assert!(std::mem::align_of::<{ty}>() == {align}, "row {n}: align of {tag} is not {align}");')
    if offs:
        for fo in offs.split(','):
            f, o = fo.split('=')
            out.append(f'const _: () = assert!(std::mem::offset_of!({ty}, {f}) == {o}, "row {n}: offset of {tag}.{f} is not {o}");')
open(sys.argv[2], 'w').write('\n'.join(out) + '\n')
print(f'{len(rows)} rows, {len(out) - 3} assertions')
