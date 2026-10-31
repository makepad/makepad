#!/usr/bin/env python3
"""Regenerates stability.tsv: the `#[unstable(feature = ..)]` items of rustc's library (library/{core,alloc,std}/src),
as `path<TAB>feature<TAB>reason`. Usage: gen_stability.py <rust-source-root> > stability.tsv

A line scanner, not a parser: it follows module / inherent impl / trait nesting by brace depth and attaches a pending
`#[unstable(..)]` to the next item header. `path` is the definition path in the real library
(`core::intrinsics::copy`, `alloc::vec::Vec::drain_filter`); Relax's std mirrors the module layout, so the same path
names its definition there (stability.rs in relax-types resolves a use to its def path and looks it up).
"""
import os, re, sys

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

def main():
    root = sys.argv[1]
    out = []
    for crate in ('core', 'alloc', 'std'):
        src = os.path.join(root, 'library', crate, 'src')
        for d, _, fs in os.walk(src):
            for f in sorted(fs):
                if f.endswith('.rs'): scan(crate, src, os.path.join(d, f), out)
    seen = set()
    for p, f, r in sorted(out):
        if (p, f) in seen: continue
        seen.add((p, f))
        print('%s\t%s\t%s' % (p, f, r))

main()
