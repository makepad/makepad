#!/bin/bash
# tandem_library.sh: fetch the hybrid synth's patch library (the same pinned archive the Synth app's opt-in
# download uses: upstream source archive of commit dab78c6, revision-31 content) into a node cache, unpack its
# resources/data/ patch folders, and run the library test against it. Run from the tree root on a pool node:
#   makepad devhub run <lane> test -- bash libs/audio_synth/tests/tandem_library.sh
set -e
SHA=dab78c6170732b3883216e569a84bf084ae0ea97
SUM=9a81357b0aca41e25b9a95f0fbfd7e9bdfbeea77b4f6533c124d4a87334da3f1
C=${TANDEM_LIBRARY_CACHE:-$HOME/synthlib}
mkdir -p "$C"
Z=$C/$SHA.zip
if [ ! -f "$Z" ] || [ "$(sha256sum "$Z" | cut -d' ' -f1)" != "$SUM" ]; then
  curl -sL -o "$Z.part" "https://github.com/surge-synthesizer/surge/archive/$SHA.zip"
  [ "$(sha256sum "$Z.part" | cut -d' ' -f1)" = "$SUM" ] || { echo "archive hash mismatch"; exit 1; }
  mv "$Z.part" "$Z"
fi
D=$C/$SHA-data
if [ ! -d "$D/patches_factory" ]; then
  python3 -I - "$Z" "$D" <<'PY'
import sys, zipfile, os
z = zipfile.ZipFile(sys.argv[1]); out = sys.argv[2]
for i in z.infolist():
    p = i.filename.split('/')
    if len(p) > 4 and p[1:3] == ['resources', 'data'] and p[3] in ('patches_factory', 'patches_3rdparty') and not i.is_dir():
        rel = os.path.join(*p[3:])
        dst = os.path.join(out, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        with open(dst, 'wb') as f:
            f.write(z.read(i))
PY
fi
TANDEM_PATCH_LIBRARY=$D cargo test --release -p makepad-audio-synth --test tandem_library -j8 -- --nocapture
# Reference renders (measurements only), when shipped beside the tree: refrender_data/{suite.txt,out/*.json}.
if [ -f refrender_data/suite.txt ]; then
  TANDEM_PATCH_LIBRARY=$D TANDEM_REFRENDERS=$PWD/refrender_data cargo test --release -p makepad-audio-synth --test tandem_refrenders -j8 -- --nocapture
fi
