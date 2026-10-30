"""Generate draw/src/text/line_break_data.rs from the Unicode Character Database; stdlib only.

The data holds what makepad_draw::text::line_break needs per code point: the UAX #14 Line_Break
class after the LB1 default resolution, East_Asian_Width F/W/H, Pi/Pf for quotation marks,
Extended_Pictographic, and the UAX #29 Grapheme_Cluster_Break and Indic_Conjunct_Break values.

Run with the files of one UCD version, for example:

    python3 -I tools/unicode_line_break/generate_line_break_data.py \\
        --line-break UCD/LineBreak.txt \\
        --east-asian-width UCD/EastAsianWidth.txt \\
        --general-category UCD/extracted/DerivedGeneralCategory.txt \\
        --emoji-data UCD/emoji/emoji-data.txt \\
        --grapheme-break UCD/auxiliary/GraphemeBreakProperty.txt \\
        --derived-core-properties UCD/DerivedCoreProperties.txt \\
        --output draw/src/text/line_break_data.rs
"""
import argparse
import itertools
import re

CODE_POINT_COUNT = 0x110000

# Line_Break values as named by UAX #14; the Rust `Class` enum has a variant for each.
LINE_BREAK_CLASSES = [
    'BK', 'CR', 'LF', 'CM', 'NL', 'WJ', 'ZW', 'GL', 'SP', 'ZWJ', 'B2', 'BA', 'BB', 'HY', 'HH',
    'CB', 'CL', 'CP', 'EX', 'IN', 'NS', 'OP', 'QU', 'IS', 'NU', 'PO', 'PR', 'SY', 'AK', 'AL',
    'AP', 'AS', 'EB', 'EM', 'H2', 'H3', 'HL', 'ID', 'JL', 'JT', 'JV', 'RI', 'VF', 'VI',
    # Resolved by LB1 below.
    'AI', 'SG', 'XX', 'SA', 'CJ',
]

# Grapheme_Cluster_Break values, mapped to the Rust `GraphemeClusterBreak` variants.
GRAPHEME_CLUSTER_BREAKS = {
    'Other': 'Other', 'CR': 'CR', 'LF': 'LF', 'Control': 'Control', 'Extend': 'Extend',
    'ZWJ': 'ZWJ', 'Regional_Indicator': 'RegionalIndicator', 'Prepend': 'Prepend',
    'SpacingMark': 'SpacingMark', 'L': 'L', 'V': 'V', 'T': 'T', 'LV': 'LV', 'LVT': 'LVT',
}

# Indic_Conjunct_Break values, mapped to the Rust flag names.
CONJUNCT_BREAK_FLAGS = {
    'Linker': 'CONJUNCT_LINKER', 'Consonant': 'CONJUNCT_CONSONANT', 'Extend': 'CONJUNCT_EXTEND',
}

DOTTED_CIRCLE = 0x25CC
NO_BREAK_SPACE = 0x00A0

# The layouter finds grapheme clusters with unicode-segmentation (Unicode 16.0), which still has
# these as Prepend, so we keep them Prepend to never break inside one of its clusters.
UNICODE_SEGMENTATION_PREPEND = [0x11A3A]

# Trie shape: the top bits pick a block of the middle index, which picks a leaf block of
# property indices. These shifts give the smallest tables for UCD 18.0.0.
MIDDLE_SHIFT = 9
LEAF_SHIFT = 4

# Values we know, to catch a parsing mistake.
EXPECTED_PROPS = {
    0x0020: ('SP', 'Other', ()),
    0x000A: ('LF', 'LF', ()),
    0x00A0: ('GL', 'Other', ('NO_BREAK_SPACE',)),
    0x00AD: ('HH', 'Control', ()),
    0x0301: ('CM', 'Extend', ('CONJUNCT_EXTEND',)),
    0x0E01: ('AL', 'Other', ()),
    0x0E31: ('CM', 'Extend', ('CONJUNCT_EXTEND',)),
    0x200D: ('ZWJ', 'ZWJ', ('CONJUNCT_EXTEND',)),
    0x201C: ('QU', 'Other', ('INITIAL_QUOTE',)),
    0x201D: ('QU', 'Other', ('FINAL_QUOTE',)),
    0x25CC: ('AL', 'Other', ('DOTTED_CIRCLE',)),
    0x3001: ('CL', 'Other', ('EAST_ASIAN',)),
    0xFFFC: ('CB', 'Other', ()),
    0x1F1E6: ('RI', 'RegionalIndicator', ()),
    0x1FFFD: ('ID', 'Other', ('UNASSIGNED_EXT_PICT', 'EXT_PICT')),
    0x11A3A: ('AL', 'Prepend', ('CONJUNCT_LINKER',)),
}


def read_ranges(path, field=1, property_name=None):
    """Yields (first, last, value) for every data line, with `@missing` defaults first."""
    with open(path, encoding='utf-8') as f:
        lines = f.read().splitlines()
    for line in lines:
        if line.startswith('# @missing:'):
            parts = [part.strip() for part in line[len('# @missing:'):].split(';')]
            if property_name is None or (len(parts) > 2 and parts[1] == property_name):
                yield parse_range(parts[0]) + (parts[-1],)
    for line in lines:
        data = line.split('#', 1)[0].strip()
        if not data:
            continue
        parts = [part.strip() for part in data.split(';')]
        if property_name is not None and (len(parts) <= 2 or parts[1] != property_name):
            continue
        yield parse_range(parts[0]) + (parts[field],)


def parse_range(text):
    first, _, last = text.partition('..')
    return int(first, 16), int(last or first, 16)


def fill(values, path, field=1, property_name=None):
    for first, last, value in read_ranges(path, field, property_name):
        values[first:last + 1] = [value] * (last - first + 1)


def format_items(values, indent='    ', width=100):
    lines, line = [], indent
    for value in values:
        item = f'{value},'
        if len(line) + len(item) + 1 > width:
            lines.append(line.rstrip())
            line = indent
        line += item + ' '
    if line.strip():
        lines.append(line.rstrip())
    return '\n'.join(lines)


def get_unsigned_type(values):
    largest = max(values)
    return 'u8' if largest <= 0xFF else 'u16' if largest <= 0xFFFF else 'u32'


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    parser.add_argument('--line-break', required=True)
    parser.add_argument('--east-asian-width', required=True)
    parser.add_argument('--general-category', required=True)
    parser.add_argument('--emoji-data', required=True)
    parser.add_argument('--grapheme-break', required=True)
    parser.add_argument('--derived-core-properties', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()

    versions = {}
    for path in [args.line_break, args.east_asian_width, args.general_category, args.emoji_data,
                 args.grapheme_break, args.derived_core_properties]:
        with open(path, encoding='utf-8') as f:
            header = ''.join(itertools.takewhile(lambda line: line.startswith('#') or not line.strip(), f))
        match = re.search(r'^#\s*(?:[A-Za-z]+-(\d+\.\d+\.\d+)\.txt|Version:\s*(\d+\.\d+\.\d+))', header, re.M)
        if not match:
            raise SystemExit(f'{path}: no UCD version in the header')
        versions[path] = match.group(1) or match.group(2)
    if len(set(versions.values())) != 1:
        raise SystemExit(f'the UCD files are from different versions: {versions}')
    version = versions[args.line_break]

    line_break = ['XX'] * CODE_POINT_COUNT
    fill(line_break, args.line_break)
    general_category = ['Cn'] * CODE_POINT_COUNT
    fill(general_category, args.general_category)
    east_asian_width = ['N'] * CODE_POINT_COUNT
    fill(east_asian_width, args.east_asian_width)
    grapheme_break = ['Other'] * CODE_POINT_COUNT
    fill(grapheme_break, args.grapheme_break)
    for code_point in UNICODE_SEGMENTATION_PREPEND:
        grapheme_break[code_point] = 'Prepend'
    conjunct_break = ['None'] * CODE_POINT_COUNT
    fill(conjunct_break, args.derived_core_properties, field=2, property_name='InCB')
    is_ext_pict = [False] * CODE_POINT_COUNT
    for first, last, value in read_ranges(args.emoji_data):
        if value == 'Extended_Pictographic':
            is_ext_pict[first:last + 1] = [True] * (last - first + 1)

    props = []
    for code_point in range(CODE_POINT_COUNT):
        class_name = line_break[code_point]
        if class_name not in LINE_BREAK_CLASSES:
            raise SystemExit(f'U+{code_point:04X}: unknown Line_Break value {class_name}')
        category = general_category[code_point]
        # LB1, with its default resolution of the classes it leaves to the implementation.
        if class_name in ('AI', 'SG', 'XX'):
            class_name = 'AL'
        elif class_name == 'SA':
            class_name = 'CM' if category in ('Mn', 'Mc') else 'AL'
        elif class_name == 'CJ':
            class_name = 'NS'
        flags = []
        if east_asian_width[code_point] in ('F', 'W', 'H'):
            flags.append('EAST_ASIAN')
        if class_name == 'QU' and category == 'Pi':
            flags.append('INITIAL_QUOTE')
        if class_name == 'QU' and category == 'Pf':
            flags.append('FINAL_QUOTE')
        if is_ext_pict[code_point] and category == 'Cn':
            flags.append('UNASSIGNED_EXT_PICT')
        if code_point == DOTTED_CIRCLE:
            flags.append('DOTTED_CIRCLE')
        if code_point == NO_BREAK_SPACE:
            flags.append('NO_BREAK_SPACE')
        if is_ext_pict[code_point]:
            flags.append('EXT_PICT')
        if conjunct_break[code_point] != 'None':
            flags.append(CONJUNCT_BREAK_FLAGS[conjunct_break[code_point]])
        props.append((class_name, GRAPHEME_CLUSTER_BREAKS[grapheme_break[code_point]], tuple(flags)))
    for code_point, value in EXPECTED_PROPS.items():
        if props[code_point] != value:
            raise SystemExit(f'U+{code_point:04X}: expected {value}, generated {props[code_point]}')

    distinct_props = list(dict.fromkeys(props))
    if len(distinct_props) > 0x100:
        raise SystemExit(f'{len(distinct_props)} distinct properties do not fit the u8 leaves')
    prop_ids = {value: index for index, value in enumerate(distinct_props)}
    indices = [prop_ids[value] for value in props]

    # Deduplicate the leaf blocks of property indices, then the middle blocks of leaf block ids.
    leaf_size = 1 << LEAF_SHIFT
    leaves, leaf_ids, middle_entries = [], {}, []
    for start in range(0, CODE_POINT_COUNT, leaf_size):
        block = tuple(indices[start:start + leaf_size])
        if block not in leaf_ids:
            leaf_ids[block] = len(leaf_ids)
            leaves.extend(block)
        middle_entries.append(leaf_ids[block])
    middle_size = 1 << (MIDDLE_SHIFT - LEAF_SHIFT)
    middle, middle_ids, top = [], {}, []
    for start in range(0, len(middle_entries), middle_size):
        block = tuple(middle_entries[start:start + middle_size])
        if block not in middle_ids:
            middle_ids[block] = len(middle_ids)
            middle.extend(block)
        top.append(middle_ids[block])

    entries = []
    for class_name, grapheme_cluster_break, flags in distinct_props:
        flags_text = ' | '.join(flags) if flags else '0'
        entries.append(
            f'    CharProps::new({class_name}, GraphemeClusterBreak::{grapheme_cluster_break}, {flags_text}),'
        )
    used_flags = list(dict.fromkeys(flag for _, _, flags in distinct_props for flag in flags))
    source = f"""// Generated from the Unicode {version} UCD by tools/unicode_line_break/generate_line_break_data.py.
// Don't edit by hand; rerun the generator instead.

use super::line_break::{{
{format_items(['CharProps', 'Class::*', 'GraphemeClusterBreak'] + used_flags)}
}};

pub(super) const MIDDLE_SHIFT: u32 = {MIDDLE_SHIFT};
pub(super) const LEAF_SHIFT: u32 = {LEAF_SHIFT};

pub(super) static PROPS: [CharProps; {len(distinct_props)}] = [
{chr(10).join(entries)}
];

pub(super) static LATIN1: [u8; 256] = [
{format_items(indices[:256])}
];

pub(super) static TOP: [{get_unsigned_type(top)}; {len(top)}] = [
{format_items(top)}
];

pub(super) static MIDDLE: [{get_unsigned_type(middle)}; {len(middle)}] = [
{format_items(middle)}
];

pub(super) static LEAVES: [u8; {len(leaves)}] = [
{format_items(leaves)}
];
"""
    with open(args.output, 'w', encoding='utf-8', newline='\n') as f:
        f.write(source)
    print(f'{args.output}: Unicode {version}, {len(distinct_props)} property sets, '
          f'{len(top)} + {len(middle)} + {len(leaves)} table entries')


if __name__ == '__main__':
    main()
