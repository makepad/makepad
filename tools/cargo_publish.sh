DRYRUN=${1:---dry-run}
#echo "---- PUBLISHING makepad-math ----"
#cargo publish $DRYRUN -p makepad-math
#echo "---- PUBLISHING makepad-error-log ----"
#cargo publish $DRYRUN -p makepad-error-log
#echo "---- PUBLISHING makepad-micro-proc-macro  ----"
#cargo publish $DRYRUN -p makepad-micro-proc-macro 
#echo "---- PUBLISHING makepad-live-id-macros ----"
#cargo publish $DRYRUN -p makepad-live-id-macros
#echo "---- PUBLISHING makepad-live-id ----"
#cargo publish $DRYRUN -p makepad-live-id
#echo "---- PUBLISHING makepad-micro-serde-derive ----"
#cargo publish $DRYRUN -p makepad-micro-serde-derive
#echo "---- PUBLISHING makepad-micro-serde ----"
#cargo publish $DRYRUN -p makepad-micro-serde
#echo "---- PUBLISHING makepad-live-tokenizer ----"
#cargo publish $DRYRUN -p makepad-live-tokenizer
#echo "---- PUBLISHING makepad-derive-live ----"
#cargo publish $DRYRUN -p makepad-derive-live
#echo "---- PUBLISHING makepad-live-compiler ----"
#cargo publish $DRYRUN -p makepad-live-compiler
#echo "---- PUBLISHING makepad-shader-compiler ----"
#cargo publish $DRYRUN -p makepad-shader-compiler
#echo "---- PUBLISHING makepad-objc-sys ----" 46
#cargo publish $DRYRUN -p makepad-objc-sys
#echo "---- PUBLISHING makepad-derive-wasm-bridge ----" 56
#cargo publish $DRYRUN -p makepad-derive-wasm-bridge
#echo "---- PUBLISHING makepad-wasm-bridge ----" 56
#cargo publish $DRYRUN -p makepad-wasm-bridge
#echo "---- PUBLISHING makepad-platform ----" 1906
#cargo publish $DRYRUN -p makepad-platform
#echo "---- PUBLISHING makepad-vector ----" 1916
#cargo publish $DRYRUN -p makepad-vector
#echo "---- PUBLISHING makepad-image-formats ----" 1926
#cargo publish $DRYRUN -p makepad-image-formats
#echo "---- PUBLISHING makepad-draw-2d ----" 1936
#cargo publish $DRYRUN -p makepad-draw-2d
#echo "---- PUBLISHING makepad-derive-widget ----" 1946
#cargo publish $DRYRUN -p makepad-derive-widget
#echo "---- PUBLISHING makepad-widgets ----" 1956
#cargo publish $DRYRUN -p makepad-widgets
#echo "---- PUBLISHING makepad-media----" 2006
#cargo publish $DRYRUN -p makepad-media
#echo "---- PUBLISHING makepad-example-ironfish----" 2016
#cargo publish $DRYRUN -p makepad-example-ironfish

#echo "---- PUBLISHING makepad-example-fractal-zoom----" 2016
#cargo publish $DRYRUN -p makepad-example-fractal-zoom

#echo "---- PUBLISHING makepad-example-simple ----" 2006
#cargo publish $DRYRUN -p makepad-example-simple
echo "---- PUBLISHING makepad-miniz----" 2026
cargo publish $DRYRUN -p makepad-miniz
echo "---- PUBLISHING makepad-base64----" 2036
cargo publish $DRYRUN -p makepad-base64
