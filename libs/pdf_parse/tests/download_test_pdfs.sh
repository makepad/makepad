#!/bin/bash
# Download the PDF test corpus from stillhq.com-pdfdb
# Usage: cd libs/pdf_parse/tests && bash download_test_pdfs.sh

set -e

DIR="$(cd "$(dirname "$0")" && pwd)"
DEST="$DIR/pdfdb"

if [ -d "$DEST" ]; then
    echo "pdfdb/ already exists — to re-download, remove it first."
    exit 0
fi

echo "Cloning PDF test corpus (may take a while)..."
git clone --depth 1 https://github.com/pdf-raku/stillhq.com-pdfdb.git "$DEST"

echo ""
echo "Done. $(find "$DEST" -name '*.pdf' | wc -l | tr -d ' ') PDF files downloaded to $DEST"
