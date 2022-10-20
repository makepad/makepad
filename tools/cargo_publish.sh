DRYRUN=${1:---dry-run}
echo "---- PUBLISHING makepad-math ----"
cargo publish $DRYRUN -p makepad-math
echo "---- PUBLISHING makepad-error-log ----"
cargo publish $DRYRUN -p makepad-error-log
echo "---- PUBLISHING makepad-micro-proc-macro  ----"
cargo publish $DRYRUN -p makepad-micro-proc-macro 
echo "---- PUBLISHING makepad-live-id-macros ----"
cargo publish $DRYRUN -p makepad-live-id-macros
echo "---- PUBLISHING makepad-live-id ----"
cargo publish $DRYRUN -p makepad-live-id
echo "---- PUBLISHING makepad-micro-serde-derive ----"
cargo publish $DRYRUN -p makepad-micro-serde-derive
echo "---- PUBLISHING makepad-micro-serde ----"
cargo publish $DRYRUN -p makepad-micro-serde
echo "---- PUBLISHING makepad-live-tokenizer ----"
cargo publish $DRYRUN -p makepad-live-tokenizer
echo "---- PUBLISHING makepad-derive-live ----"
cargo publish $DRYRUN -p makepad-derive-live
echo "---- PUBLISHING makepad-live-compiler ----"
cargo publish $DRYRUN -p makepad-live-compiler
echo "---- PUBLISHING makepad-shader-compiler ----"
cargo publish $DRYRUN -p makepad-shader-compiler
echo "---- PUBLISHINGmakepad-objc-sys ----"
cargo publish $DRYRUN -p makepad-objc-sys
echo "---- PUBLISHING makepad-platform ----"
cargo publish $DRYRUN -p makepad-platform
echo "---- PUBLISHINGmakepad-internal-iter ----"
cargo publish $DRYRUN -p makepad-internal-iter
echo "---- PUBLISHING makepad-geometry ----"
cargo publish $DRYRUN -p makepad-geometry
echo "---- PUBLISHINGmakepad-path ----"
cargo publish $DRYRUN -p makepad-path
echo "---- PUBLISHING makepad-font ----"
cargo publish $DRYRUN -p makepad-font
echo "---- PUBLISHING makepad-image-formats ----"
cargo publish $DRYRUN -p makepad-image-formats
echo "---- PUBLISHING makepad-trapezoidator ----"
cargo publish $DRYRUN -p makepad-trapezoidator
echo "---- PUBLISHING makepad-font ----"
cargo publish $DRYRUN -p makepad-font
echo "---- PUBLISHING makepad-ttf-parser ----"
cargo publish $DRYRUN -p makepad-ttf-parser
echo "---- PUBLISHING makepad-draw-2d ----"
cargo publish $DRYRUN -p makepad-draw-2d
echo "---- PUBLISHING makepad-derive-widget ----"
cargo publish $DRYRUN -p makepad-derive-widget
echo "---- PUBLISHING makepad-widgets ----"
cargo publish $DRYRUN -p makepad-widgets
echo "---- PUBLISHING makepad-example-simple ----"
cargo publish $DRYRUN -p makepad-example-simple

