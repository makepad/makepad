#!/usr/bin/env python3
"""Regenerates stability.tsv: the `#[unstable(feature = ..)]` items of rustc's library (library/{core,alloc,std}/src),
as `path<TAB>feature<TAB>reason`. Usage: gen_stability.py <rust-source-root> > stability.tsv

`gen_stability.py --stabilized <rust-source-root> < stability.tsv > new.tsv` moves the current table to a newer
toolchain's library without a full regeneration (whose paths churn when the library moves code between crates): a
row whose feature the new library no longer gates anywhere, and names in some `#[stable(feature = ..)]`, becomes
that path's row in the new library (stable, or the feature it is gated under now); every other row stays.

`gen_stability.py --surface-check <makepad> <work dir>` checks the stable items in stability.tsv against rustc.
It tries public spellings for each definition path, keeps the first spelling rustc accepts on this host, and
checks the same references in a package. A missing item fails the check. unmapped.txt lists each path outside
the comparison and the reason it was left out.

A line scanner, not a parser: it follows module / inherent impl / trait nesting by brace depth and attaches a pending
`#[unstable(..)]` to the next item header. `path` is the definition path in the real library
(`core::intrinsics::copy`, `alloc::vec::Vec::drain_filter`); Relax's std mirrors the module layout, so the same path
names its definition there (stability.rs in relax-types resolves a use to its def path and looks it up).
"""
import os, re, sys
import argparse
import subprocess
from pathlib import Path

ITEM = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:unsafe\s+)?(?:async\s+)?(?:extern\s+"[^"]*"\s+)?(struct|enum|union|trait|fn|mod|type|const|static)\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)')
IMPL = re.compile(r'^\s*(?:unsafe\s+)?impl\b(.*)')
MACRO = re.compile(r'^\s*(?:pub\s+)?macro\s+([A-Za-z_][A-Za-z0-9_]*)')

def strip_comments(src):
    out = []
    in_block = 0
    for line in src.split('\n'):
        res = ''
        i = 0
        while i < len(line):
            if in_block:
                j = line.find('*/', i)
                if j < 0: i = len(line)
                else: in_block -= 1; i = j + 2
            elif line.startswith('/*', i):
                in_block += 1; i += 2
            elif line.startswith('//', i):
                break
            else:
                res += line[i]; i += 1
        out.append(res)
    return out

def module_path(crate, root, f):
    rel = os.path.relpath(f, root)[:-3].split(os.sep)
    if rel[-1] in ('mod', 'lib'): rel = rel[:-1]
    return [crate] + rel

def scan(crate, root, f, out):
    lines = strip_comments(open(f, encoding='utf-8', errors='replace').read())
    base = module_path(crate, root, f)
    stack = []  # (depth_after_open, kind, name)
    depth = 0
    pending = None  # (feature, reason)
    attr = None     # accumulating attribute text
    for line in lines:
        s = line.strip()
        if attr is not None:
            attr += ' ' + s
            if ']' in s and attr.count('[') <= attr.count(']'):
                pending = file_attr(attr, base, stack, out) or pending
                attr = None
        elif s.startswith('#[stable'):
            pending = ('', '')
        elif s.startswith('#[unstable') or s.startswith('#![unstable'):
            attr = s
            if attr.count('[') <= attr.count(']'):
                pending = file_attr(attr, base, stack, out) or pending
                attr = None
        opened = None
        if attr is None and s and not s.startswith('#['):
            m = ITEM.match(line)
            in_trait_impl = any(k == 'impl_for' for _, k, _ in stack)
            if m:
                kind, name = m.group(1), m.group(2)
                # `pub mod x {}` under cfg(doc ..): a documentation placeholder, not the module
                placeholder = kind == 'mod' and re.search(r'\{\s*\}', line)
                if pending and not in_trait_impl and not placeholder:
                    path = '::'.join(base + [n for _, k, n in stack if k in ('mod', 'type', 'trait')] + [name])
                    out.append((path, pending[0], pending[1]))
                if kind == 'mod' and '{' in line: opened = ('mod', name)
                elif kind == 'trait' and '{' in line: opened = ('trait', name)
                pending = None
            else:
                mi = IMPL.match(line)
                if mi:
                    rest = mi.group(1)
                    if re.search(r'\bfor\b', rest.split('{')[0]):
                        opened = ('impl_for', '')
                    else:
                        body = re.sub(r'^\s*<(?:[^<>]|<(?:[^<>]|<[^<>]*>)*>)*>\s*', '', rest)
                        tn = re.match(r'\s*[&*]?\s*(?:mut\s+|const\s+)?(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)', body)
                        opened = ('type', tn.group(1) if tn else '') if '{' in line else None
                    pending = None
                elif pending and re.match(r'^(?:pub(?:\([^)]*\))?\s+)?use\b', s):
                    # a re-export carries its own stability: `pub use a::b::c [as d];`
                    mu = re.match(r'^(?:pub(?:\([^)]*\))?\s+)?use\s+([A-Za-z0-9_:]*?([A-Za-z_][A-Za-z0-9_]*))(?:\s+as\s+([A-Za-z_][A-Za-z0-9_]*))?\s*;', s)
                    if mu and not in_trait_impl:
                        where = '::'.join(base + [n for _, k, n in stack if k in ('mod', 'type', 'trait')] + [mu.group(3) or mu.group(2)])
                        if pending[0]:
                            out.append((where, pending[0], pending[1]))
                        else:
                            # a stable re-export takes the stability of its target (rustc resolves to the definition)
                            tgt = mu.group(1)
                            if tgt.split('::')[0] in ('core', 'alloc', 'std'):
                                out.append((where, '', 'reexport ' + tgt))
                    pending = None
                elif pending and (MACRO.match(line) or s.startswith('macro_rules!') or s.startswith('#')):
                    pass
                elif pending:
                    pending = None   # a field, a statement ..: the attribute was not on an item
        o, c = line.count('{'), line.count('}')
        if opened and o > 0:
            stack.append((depth + 1, opened[0], opened[1]))
        depth += o - c
        while stack and depth < stack[-1][0]:
            stack.pop()

def file_attr(a, base, stack, out):
    """An inner `#![unstable(..)]` at the top of a file marks the file's module; an outer one is pending."""
    p = parse_attr(a)
    if p and a.startswith('#![') and not stack:
        if len(base) > 1: out.append(('::'.join(base), p[0], p[1]))
        return None
    return p

def parse_attr(a):
    m = re.search(r'feature\s*=\s*"([^"]+)"', a)
    if not m: return None
    r = re.search(r'reason\s*=\s*"((?:[^"\\]|\\.)*)"', a)
    return (m.group(1), re.sub(r'\\\s+', '', r.group(1)).replace('\t', ' ') if r else '')

def library_files(root):
    for crate in ('core', 'alloc', 'std'):
        src = os.path.join(root, 'library', crate, 'src')
        for d, _, fs in os.walk(src):
            for f in sorted(fs):
                if f.endswith('.rs'): yield crate, src, os.path.join(d, f)

def table(root):
    out = []
    for crate, src, f in library_files(root): scan(crate, src, f, out)
    seen = set()
    rows = []
    for p, f, r in sorted(out):
        if (p, f) in seen: continue
        seen.add((p, f))
        rows.append((p, f, r))
    return rows

def stabilized(root):
    """Features some `#[stable]` names and no `#[unstable]` (outside comments) gates any more."""
    stable, unstable = set(), set()
    for _, _, f in library_files(root):
        text = '\n'.join(strip_comments(open(f, encoding='utf-8', errors='replace').read()))
        stable.update(re.findall(r'#\[stable\(\s*feature\s*=\s*"([^"]+)"', text))
        unstable.update(re.findall(r'#!?\[unstable\(\s*feature\s*=\s*"([^"]+)"', text))
    return stable - unstable

PRIMITIVES = {'f32', 'f64', 'str', 'char', 'u8', 'i8'}
EXISTS = {'E0282', 'E0283', 'E0284', 'E0790'}

def host_os_modules():
    cfg = subprocess.check_output(['rustc', '--print', 'cfg'], text=True)
    modules = set(re.findall(r'^target_(?:os|family)="([^"]+)"$', cfg, re.M))
    modules.add('raw')
    if 'target_vendor="apple"' in cfg:
        modules.add('darwin')
    if modules.intersection({'unix', 'hermit', 'trusty', 'wasi', 'motor'}):
        modules.add('fd')
    if modules.intersection({'linux', 'android', 'cygwin'}):
        modules.add('net')
    if 'solid_asp3' in modules:
        modules.add('solid')
    if 'qnx' in modules:
        modules.add('nto')
    return modules

def surface_exclusion(path, os_modules):
    parts = path.split('::')
    if parts[:2] == ['std', 'os'] and len(parts) > 2 and parts[2] not in os_modules:
        return 'other-os'
    if path == 'std::os::unix::fs::MetadataExt::attrib' and 'vxworks' not in os_modules:
        return 'other-os'
    if path in ('core::ffi::va_list', 'core::slice::index::private_slice_index::Sealed',
                'std::sync::poison::mutex') or path.endswith(('::RcEqIdent', '::ArcEqIdent', '::RawWaker::NOOP')):
        return 'private'
    if parts[:-1] == ['core', 'cmp'] and parts[-1] in ('ge', 'gt', 'le', 'lt', 'partial_cmp'):
        return 'assoc-type'
    # rustc refuses any reference to Drop::drop (E0040, explicit destructor call)
    if path == 'core::ops::drop::Drop::drop':
        return 'unnameable'
    # std::os::windows::ffi re-exports it
    if parts[:2] == ['core', 'wtf8'] and 'windows' not in os_modules:
        return 'other-os'
    # std::os::net is private; std::os::linux::net and std::os::android::net re-export its items
    if parts[:4] == ['std', 'os', 'net', 'linux_ext'] and len(parts) == 5:
        return 'private'

def surface_reference(path):
    parts = path.split('::')
    if 'dyn' in parts:
        return None
    root = 'core' if parts[0] == 'core' else 'std'
    module, parent, name = parts[1], parts[-2], parts[-1]
    if '' in parts:
        # An empty owner in core::slice is an inherent impl on [T].
        owner = {'core::slice': '<[u8]>', 'core::mem::maybe_uninit': '<[core::mem::MaybeUninit<u8>]>',
                 'core::sync::atomic': 'core::sync::atomic::AtomicU8'}.get('::'.join(parts[:parts.index('')]))
        return f'{owner}::{name}' if owner else None
    if parent == 'T':
        owner = {'core::ptr::const_ptr': '<*const u8>', 'core::ptr::mut_ptr': '<*mut u8>',
                 'core::cell::lazy': 'core::cell::LazyCell::<u8>',
                 'std::sync::lazy_lock': 'std::sync::LazyLock::<u8>'}.get('::'.join(parts[:-2]))
        if owner:
            return f'{owner}::{name}'
    if parent in ('uint_macros', 'int_macros'):
        return ('u8' if parent == 'uint_macros' else 'i8') + '::' + name
    if parts[:2] != ['alloc', 'str']:
        if parent in PRIMITIVES:
            return f'{parent}::{name}'
        if len(parts) >= 3 and parent[0].isupper():
            owner = f'{root}::{module}::{parent}'
            if module == 'os':
                owner = '::'.join(['std'] + parts[1:-1])
            elif parts[1:3] == ['sync', 'atomic']:
                owner = f'{root}::sync::atomic::{parent}'
            elif parts[:3] == ['core', 'io', 'util']:
                owner = f'std::io::{parent}'
            return f'{owner}::{name}'
    public = '::'.join([root] + parts[1:])
    if len(parts) == 4 and name[0].isupper() and module in ('panic', 'range', 'str'):
        public = f'{root}::{module}::{name}'
    return f'use {public} as _;'

COLLECTION_MODULES = {
    'alloc::collections::btree::map': 'std::collections::btree_map',
    'alloc::collections::btree::set': 'std::collections::btree_set',
    'std::collections::hash::map': 'std::collections::hash_map',
    'std::collections::hash::set': 'std::collections::hash_set',
}
MEMBER_OWNERS = {
    'alloc::slice::': ('<[u8]>', '<[[u8; 1]]>'),
    'core::slice::': ('<[u8]>', '<[[u8; 1]]>'),
    'core::str::lossy::': ('<[u8]>',),
    'core::num::u8::': ('u8',),
    'core::result::Result': ('core::result::Result::<&u8, u8>', 'core::result::Result::<&mut u8, u8>'),
    'core::slice::ascii::': ('<[u8]>',),
    'alloc::str::str': ('str',),
    'core::array::': ('<[u8; 1]>',),
    'core::bool::bool': ('bool',),
    'core::any::dyn': ('<dyn core::any::Any>',),
    'core::error::dyn': ('<dyn core::error::Error>',),
    'alloc::boxed::convert::dyn': ('Box::<dyn core::any::Any>',),
    'core::ptr::const_ptr::const': ('<*const [u8]>',),
    'core::ptr::mut_ptr::mut': ('<*mut [u8]>',),
    'core::num::nonzero': ('core::num::NonZero::<u8>', 'core::num::NonZero::<i8>'),
    'core::fmt::num::i128': ('i128',),
    'core::fmt::num::u128': ('u128',),
    'core::fmt::num::': ('u8',),
    'core::mem::drop_guard': ('core::mem::DropGuard', 'core::mem::DropGuard::<u8, fn(u8)>'),
}

def public_references(parts):
    parent, name = '::'.join(parts[:-1]), parts[-1]
    # private OS modules, named through the public modules that re-export their items
    if parts[:4] == ['std', 'os', 'darwin', 'raw'] and len(parts) == 5:
        yield f'use std::os::macos::raw::{name} as _;'
    if parts[:4] == ['std', 'os', 'net', 'linux_ext'] and len(parts) > 5:
        yield ('use std::os::linux::net::' + parts[5] + ' as _;' if len(parts) == 6
               else 'std::os::linux::net::' + '::'.join(parts[5:]))
    if name not in ('NonZero', 'DropGuard'):
        for owner in MEMBER_OWNERS.get(parent, ()):
            yield f'{owner}::{name}'
    if parts[:2] == ['std', 'panicking'] and name in ('set_hook', 'take_hook'):
        yield f'use std::panic::{name} as _;'
    if parts[:3] in (['core', 'num', 'f32'], ['core', 'num', 'f64']):
        if parts[3:4] == parts[2:3]:
            yield '::'.join(parts[3:])
        else:
            yield 'use core::' + '::'.join(parts[2:]) + ' as _;'
    module = COLLECTION_MODULES.get('::'.join(parts[:4]))
    if module and len(parts) > 4:
        tail = parts[4:]
        # btree_map re-exports the types from its entry module.
        if tail[0] == 'entry' and len(tail) > 1:
            tail = tail[1:]
        if len(tail) == 1 and name[0].islower() and parts[2] == 'hash':
            owner = 'HashMap::<u8, u8>' if parts[3] == 'map' else 'HashSet::<u8>'
            yield f'{module}::{owner}::{name}'
        elif len(tail) > 1 and tail[-2][0].isupper():
            yield '::'.join([module] + tail)
        else:
            yield 'use ' + '::'.join([module] + tail) + ' as _;'

def surface_alternatives(path, reference=None, codes=()):
    parts = path.split('::')
    yield from public_references(parts)
    if 'E0034' in codes:
        owner, _, name = reference.rpartition('::')
        for arg in ('u8', 'i8', '&u8', 'core::mem::MaybeUninit<u8>', 'Result<u8, u8>',
                    'core::cell::Cell<u8>', 'dyn core::any::Any'):
            yield f'{owner}::<{arg}>::{name}'
        return
    if 'E0782' in codes:
        owner, _, name = reference.rpartition('::')
        yield f'<_ as {owner}>::{name}'
        return
    if {'const', 'mut', 'impl', 'fn', 'for', 'dyn', 'self', 'super', 'crate', ''}.intersection(parts):
        return
    start = next((i for i in range(1, len(parts)) if parts[i][0].isupper()), len(parts) - 1)
    for root in (('core', 'std') if parts[0] == 'core' else ('std',)):
        # Try the definition, then the re-exports above its private modules.
        # An OS-specific item must still belong to its OS module.
        stop = 2 if parts[:2] == ['std', 'os'] else 0
        for end in range(start, stop, -1):
            candidate = [root] + parts[1:end] + parts[start:]
            public = '::'.join(candidate)
            # a module named like a primitive (core::char) holds types too: those are imported
            camel = candidate[-1][0].isupper() and any(c.islower() for c in candidate[-1])
            if candidate[-2][0].isupper() or (candidate[-2] in PRIMITIVES | {'f16', 'f128'} and not camel):
                yield public
            else:
                yield f'use {public} as _;'

def surface_errors(command, cwd, source, rows):
    run = subprocess.run(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    errors = {}
    pending = None
    for line in run.stdout.splitlines():
        # Both "error[E....]: message" followed by "--> file:line:col" and
        # "file:line:col: E....: message" identify the reference on that line.
        direct = re.fullmatch(r'(?:error: )?\s*(.+?):(\d+):\d+:\s*(.*)', line)
        if direct:
            file, number, message = direct.groups()
            if message.startswith('warning:'):
                continue
            code = re.match(r'(E\d{4}):?\s*', message)
            diagnostic = (code[1], message[code.end():]) if code else ('E-', message)
        else:
            header = re.match(r'^(?:error|.*: error)(?:\[(E\d{4})\])?: (.*)', line)
            if header:
                if pending:
                    raise ValueError(f'unlocated error: {pending[1]}')
                if not re.match(r'error: (?:aborting due to |could not compile .+ due to \d+ previous error)', line):
                    pending = (header[1] or 'E-', header[2])
                continue
            location = re.match(r'^\s*--> (.*?):(\d+)(?::|$)', line)
            if not location or not pending:
                continue
            file, number = location.groups()
            diagnostic, pending = pending, None
        if (file != source.name and (cwd / file).resolve() != source) or not 2 <= int(number) <= rows + 1:
            raise ValueError(f'error outside the references: {line}: {diagnostic[1]}')
        errors.setdefault(int(number), []).append(diagnostic)
    if pending:
        raise ValueError(f'unlocated error: {pending[1]}')
    if run.returncode not in (0, 1) or (run.returncode and not errors):
        raise ValueError(f'{command[0]} exited {run.returncode}\n{run.stdout}')
    return errors

def rustc_references(work, stem, groups):
    source = work / (stem + '.rs')
    lines = ['#![allow(unused_imports, deprecated, dead_code, path_statements)]']
    refs = []
    for path, spellings in groups.items():
        for k, spelling in enumerate(spellings):
            body = spelling if spelling.startswith('use ') else f'fn r{len(refs)}() {{ let _ = {spelling}; }}'
            label = f'alt {k} row' if stem == 'alts' else 'row'
            lines.append(f'{body} // {label} {path}')
            refs.append((path, spelling))
    source.write_text('\n'.join(lines) + '\n')
    command = ['rustc', '--edition', '2021', '--crate-type', 'lib', '--emit=metadata',
               '-o', stem + '.rmeta', source.name]
    errors = surface_errors(command, work, source, len(refs))
    return [(path, spelling, [e for e in errors.get(n, ()) if e[0] not in EXISTS])
            for n, (path, spelling) in enumerate(refs, 2)]

def surface_check(binary, work, rustc_only):
    work.mkdir(parents=True, exist_ok=True)
    rows = []
    first = {}
    reasons = {}
    alternatives = {}
    os_modules = host_os_modules()
    for line in Path(__file__).with_name('stability.tsv').read_text().splitlines():
        path, feature, reason = line.split('\t')
        if feature or reason.startswith('reexport '):
            continue
        rows.append(path)
        reason = surface_exclusion(path, os_modules)
        if reason:
            reasons[path] = reason
            continue
        reference = surface_reference(path)
        if reference:
            first[path] = [reference]
        else:
            alternatives[path] = surface_alternatives(path)

    chosen = {}
    for path, reference, errors in rustc_references(work, 'refs', first):
        if errors:
            alternatives[path] = surface_alternatives(path, reference, {code for code, _ in errors})
        else:
            chosen[path] = reference
    first_count = len(chosen)
    for path, reference, errors in rustc_references(work, 'alts', alternatives):
        if not errors:
            chosen.setdefault(path, reference)
        elif any(code == 'E0575' for code, _ in errors):
            # <_ as Trait>::Item in value position names an associated type.
            reasons[path] = 'assoc-type'
    unmapped = [p for p in rows if p not in chosen]
    (work / 'unmapped.txt').write_text(''.join(f"{p}\t{reasons.get(p, 'no-spelling')}\n" for p in unmapped))
    print(f'surface: {len(rows)} rows, {first_count} accepted by pass 1, '
          f'{len(chosen) - first_count} by an alternative, {len(unmapped)} unmapped')
    final = {p: [chosen[p]] for p in rows if p in chosen}
    control = rustc_references(work, 'final', final)
    for n, (path, reference, errors) in enumerate(control, 2):
        if errors:
            print(f'surface: final.rs:{n} {path} {reference} {errors[0][0]} {errors[0][1]}')
    if any(errors for _, _, errors in control):
        return 2
    if rustc_only:
        return 0

    pkg = work / 'pkg'
    src = pkg / 'src' / 'lib.rs'
    src.parent.mkdir(parents=True, exist_ok=True)
    (pkg / 'Cargo.toml').write_text('[package]\nname = "surface"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n')
    src.write_bytes((work / 'final.rs').read_bytes())
    errors = surface_errors([binary, 'check', '--no-serve', 'surface'], pkg, src, len(final))
    missing = False
    for n, path in enumerate(final, 2):
        for code, message in errors.get(n, ()):
            if code not in EXISTS:
                print(f'surface: missing {path} {code} {message[:80]}')
                missing = True
                break
    if missing:
        return 1
    print(f'surface: ok {len(final)} rows checked, {len(unmapped)} out of the comparison')
    return 0

def main():
    if sys.argv[1] == '--surface-check':
        parser = argparse.ArgumentParser(prog=f'{sys.argv[0]} --surface-check')
        parser.add_argument('makepad', type=Path)
        parser.add_argument('work', type=Path)
        parser.add_argument('--rustc-only', action='store_true', help=argparse.SUPPRESS)
        args = parser.parse_args(sys.argv[2:])
        try:
            return surface_check(args.makepad.resolve(), args.work.resolve(), args.rustc_only)
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(f'surface: {error}', file=sys.stderr)
            return 2
    if sys.argv[1] == '--stabilized':
        root = sys.argv[2]
        done = stabilized(root)
        new = {}
        for p, f, r in table(root):
            if f or p not in new: new[p] = (f, r)
        for l in sys.stdin.read().splitlines():
            p, f, r = (l.split('\t') + ['', ''])[:3]
            if f in done:
                f, r = new.get(p, ('', ''))
            print('%s\t%s\t%s' % (p, f, r))
        return
    for p, f, r in table(sys.argv[1]):
        print('%s\t%s\t%s' % (p, f, r))

sys.exit(main())
