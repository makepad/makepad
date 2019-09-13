
cargo build -p makepad_webgl --release --target=wasm32-unknown-unknown
./bin/wasm-strip ./target/wasm32-unknown-unknown/release/makepad_webgl.wasm
echo "Zipped size:";gzip -9 < ./target/wasm32-unknown-unknown/release/makepad_webgl.wasm|wc -c

#cargo build --target=wasm32-unknown-unknown --release --manifest-path="./webgl/Cargo.toml"
#node ./build_index.js
#cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html
#./bin/wasm-strip ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html
#rm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html.br
#./bin/brotli ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html
#ls -al ./webgl/target/wasm32-unknown-unknown/release/|grep makepad_webgl.webassembly.html
#echo "Zipped size:";gzip -9 < ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html|wc -c

#-s'twiggy top -n 20 ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html'
#-s'../binaryen/bin/wasm-opt -all -Oz -o ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ' \
#-s'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#buggy.
#-s'../binaryen/bin/wasm-opt -all -Oz -o ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ' \
#-s'ls -al ./webgl/target/wasm32-unknown-unknown/release/|grep makepad_webgl.webassembly' \
#-s'echo "Running Wasm opt..."'\
#-s'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#-s 'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#cargo-watch watch -x 'build --target=wasm32-unknown-unknown --manifest-path="./webgl/Cargo.toml"' -s 'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' -s 'node ./build_index.js'
