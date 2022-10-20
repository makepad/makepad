DRYRUN=${1:---dry-run}
cargo publish $DRYRUN -p makepad-math
cargo publish $DRYRUN -p makepad-error-log
cargo publish $DRYRUN -p makepad-micro-proc-macro 
cargo publish $DRYRUN -p makepad-live-id-macros
cargo publish $DRYRUN -p makepad-live-id
cargo publish $DRYRUN -p makepad-micro-serde-derive
cargo publish $DRYRUN -p makepad-micro-serde
cargo publish $DRYRUN -p makepad-live-tokenizer
cargo publish $DRYRUN -p makepad-derive-live
cargo publish $DRYRUN -p makepad-live-compiler
cargo publish $DRYRUN -p makepad-shader-compiler
cargo publish $DRYRUN -p makepad-objc-sys
cargo publish $DRYRUN -p makepad-platform
cargo publish $DRYRUN -p makepad-internal-iter
cargo publish $DRYRUN -p makepad-geometry
cargo publish $DRYRUN -p makepad-path
cargo publish $DRYRUN -p makepad-font
cargo publish $DRYRUN -p makepad-image-formats
cargo publish $DRYRUN -p makepad-trapezoidator
cargo publish $DRYRUN -p makepad-font
cargo publish $DRYRUN -p makepad-ttf-parser
cargo publish $DRYRUN -p makepad-draw-2d
cargo publish $DRYRUN -p makepad-derive-widget
cargo publish $DRYRUN -p makepad-widgets
cargo publish $DRYRUN -p makepad-example-simple

