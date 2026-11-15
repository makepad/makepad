#!/bin/bash
# usage: run.sh <makepad binary>: an item's outer doc comments reach its derive and attribute
# macros as `#[doc]` attributes (as rustc 1.93 prints them); prints "derive_doc_input: ok" or
# "derive_doc_input: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
ERR=$(mktemp)
trap 'rm -f "$ERR"' EXIT
export RELAX_CACHE=off
OUT=$(cd "$H" && RELAX_STD=$H/../../std "$1" run --quiet derive-doc-app 2>>"$ERR")
echo "$OUT"
WANT='# Bracket[doc = " One line."] # Bracket[doc = ""] # Bracket[doc = " A ` `code` ` and \"quotes\"."] pub struct Outer Brace[# Bracket[doc = " a field"] pub x : u8 ,]
# Bracket[doc = " A block comment. "] struct Block ;
# Bracket[doc = "explicit"] struct Explicit ;
# Bracket[doc = " Attr."] struct Attr ;
# Bracket[doc = " Local."] struct Local ;'
if [ "$OUT" = "$WANT" ]; then echo "derive_doc_input: ok"; else tail -5 "$ERR"; echo "derive_doc_input: FAILED"; exit 1; fi
