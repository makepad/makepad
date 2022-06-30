#!/usr/bin/python3

"""Generating Unicode tables in Rust source code"""

import argparse
import re

# The largest valid Unicode code point
MAX_CODE_POINT = 0x10FFFF


class Error(Exception):
    """Error base class."""


def parse_code_point(string):
    """Parses a Unicode code point.

    Code points are expressed as hexadecimal numbers with four to six digits.
    """
    if len(string) < 4 or len(string) > 6:
        raise Error("invalid code point %s" % string)
    try:
        code_point = int(string, 16)
    except ValueError:
        raise Error("invalid code point %s" % string)
    if code_point < 0 or code_point > MAX_CODE_POINT:
        raise Error("invalid code point %s" % string)
    return code_point


def parse_code_point_range(string):
    """Parses a range of Unicode code points.

    A range of code points is specified either by the form "X..Y", where X is
    less than or equal to Y, or by the form "X", which is short for "X..X".
    """
    bounds = string.split("..")
    if len(bounds) == 1:
        code_point = parse_code_point(bounds[0])
        return range(code_point, code_point + 1)
    elif len(bounds) == 2:
        first_code_point = parse_code_point(bounds[0])
        last_code_point = parse_code_point(bounds[1])
        if first_code_point > last_code_point:
            raise Error("invalid code point range %s" % string)
        return range(first_code_point, last_code_point + 1)
    else:
        raise Error("invalid code point range %s" % string)


def parse_character_name(string):
    """Parses a Unicode character name.

    For backward compatibility, ranges in the file UnicodeData.txt are
    specified by entries for the start and end characters of the range, rather
    than by the form "X..Y". The start character is indicated by a range
    identifier, followed by a comma and the string "First", in angle brackets.
    This line takes the place of a regular character name in field 1 for that
    line. The end character is indicated on the next line with the same range
    identifier, followed by a comma and the string "Last", in angle brackets.
    """
    match = re.match("<(.*), (First|Last)>", string)
    if match is not None:
        return match.groups()
    return (string, None)


def read_unicode_data(filename, expected_field_count):
    """A reader for Unicode data files.

    The reader strips out comments and whitespace, and skips empty lines. For
    non-empty lines, the reader splits the line into fields, checks if the
    line has the expected number of fields, and strips out whitespace for each
    field.

    The reader also takes care of parsing code point ranges. Unicode data
    files specify these in two different ways, either by the form "X..Y", or
    by entries for the start and end characters of the range.
    """
    file = open(filename, encoding="utf8")
    lineno = 1
    first = None
    expected_name = None
    for line in file:
        # Strip out comments and whitespace, and skip empty lines.
        hash = line.find("#")
        if hash >= 0:
            line = line[:hash]
        line = line.strip()
        if not line:
            continue

        try:
            # Split the line into fields, check if the line has the expected
            # number of fields, and strip out whitespace for each field.
            fields = [field.strip() for field in line.split(";")]
            if len(fields) != expected_field_count:
                raise ValueError("invalid number of fields %d" % len(fields))

            # Unicode data files specify code point ranges in two different
            # ways, either by the form "X..Y", or by entries for the start and
            # end characters of the range. 
            code_points = parse_code_point_range(fields[0])
            (name, first_or_last) = parse_character_name(fields[1])
            if expected_name is None:
                if first_or_last == "First":
                    # The current line is the first entry for a range.
                    # Remember it and continue with the next line.
                    if len(code_points) != 1:
                        raise ValueError("invalid First line")
                    expected_name = name
                    first = code_points[0]
                    continue
            else:
                # If the previous line was the first entry for a range, the
                # current line should be the last entry for that range.
                if name != expected_name or first_or_last != "Last" or len(
                        code_points) != 1 or code_points[0] < first:
                    raise ValueError("invalid Last line")
                code_points = range(first, code_points[0] + 1)
                fields[1] = name
                first = None
                expected_name = None
        except Exception as e:
            e.args = ("%s:%d:" % (filename, lineno), *e.args)
            raise e.with_traceback(e.__traceback__)   
        fields[0] = code_points
        yield fields
        lineno += 1


def group_code_points(code_points):
    """Groups a list of code points into a list of code point ranges."""

    code_point_ranges = []
    first = None
    last = None
    for code_point in sorted(code_points):
        if first is not None:
            if code_point == last + 1:
                last = code_point
                continue
            code_point_ranges.append(range(first, last + 1))
        first = code_point
        last = code_point
    if first is not None:
        code_point_ranges.append(range(first, last + 1))
    return code_point_ranges


def load_property_data(filename):
    """Loads data for a property from a Unicode data file.

    For properties, returns a dict mapping values to a list of code point
    ranges for which the property has that value.

    For binary properties, returns a dict mapping names to a list of code
    point ranges for which the property with that name has the value "True".
    """
    property = {}
    for [code_points, value] in read_unicode_data(filename, 2):
        property.setdefault(value, []).extend(code_points)
    for value in property:
        property[value] = group_code_points(property[value])
    return property


def print_property_table(name, property, default_value):
    """Prints the table for a property.

    The property is specified by a dict mapping values to a list of code point
    ranges for which the property has that value. All code points not
    explicitly listed have the given default value.
    """

    print("//! This file was generated by:")
    print("//! generate_table.py %s <ucd_dir>" % name)
    print("")
    print("use %s::*;" % name.replace("_", ""))
    print("")
    print("#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]")
    print("#[repr(u8)]")
    print("pub enum %s {" % name.replace("_", ""))
    print("    %s," % default_value.replace("_", ""))
    for variant in sorted(property):
        print("    %s," % variant.replace("_", ""))
    print("}")
    print("")
    print("impl Default for %s {" % name.replace("_", ""))
    print("    fn default() -> Self {")
    print("        Self::%s" % default_value.replace("_", ""))
    print("    }")
    print("}")
    print("")

    entries = []
    for value in property:
        entries.extend([(code_points[0], code_points[-1], value)
                       for code_points in property[value]])
    entries.sort()
    
    print("pub(crate) static %s: [([u8; 3], [u8; 3], %s); %d] = [" %
          (name.upper(), name.replace("_", ""), len(entries)))
    for (first, last, value) in entries:
        print(
            "    ([0x%02X, 0x%02X, 0x%02X], [0x%02X, 0x%02X, 0x%02X], %s)," %
            (*first.to_bytes(3, byteorder="big"),
             *last.to_bytes(3, byteorder="big"),
             value.replace("_", "")))
    print("];")


def print_binary_property_table(name, property):
    """Prints the table for a binary property.
    
    The property is specified as the list of code point ranges for which the
    property has the value "True".
    """
    
    print("//! This file was generated by:")
    print("//! generate_table.py %s <ucd_dir>" % name)
    print("")
    entries = [(code_points[0], code_points[-1]) for code_points in property]
    entries.sort()
    print("pub(crate) static %s: [([u8; 3], [u8; 3]); %d] = [" % (name.upper(), len(entries)))
    for (first, last) in entries:
        print(
            "    ([0x%02X, 0x%02X, 0x%02X], [0x%02X, 0x%02X, 0x%02X])," %
            (*first.to_bytes(3, byteorder="big"),
             *last.to_bytes(3, byteorder="big"),))
    print("];")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("name", type=str)
    parser.add_argument("ucd_dir", type=str)
    args = parser.parse_args()
    if args.name == "Extended_Pictographic":
        print_binary_property_table(args.name, load_property_data(
            args.ucd_dir + "/emoji/emoji-data.txt")["Extended_Pictographic"])
    elif args.name == "Grapheme_Cluster_Break":
        print_property_table(args.name, load_property_data(
            args.ucd_dir + "/auxiliary/GraphemeBreakProperty.txt"), "Other")
    elif args.name == "Word_Break":
        print_property_table(args.name, load_property_data(
            args.ucd_dir + "/auxiliary/WordBreakProperty.txt"), "Other")
    else:
        raise Error("invalid property name")

if __name__ == '__main__':
    main()
