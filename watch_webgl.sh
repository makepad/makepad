cargo-watch watch -x 'build --target=wasm32-unknown-unknown --release --manifest-path="./webgl/Cargo.toml"' \
-s'node ./build_index.js' \
-s'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html' \
-s'../wabt/bin/wasm-strip ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html' \
-s'ls -al ./webgl/target/wasm32-unknown-unknown/release/|grep makepad_webgl.webassembly.html' \
-s'echo "Zipped size:";gzip -9 < ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html|wc -c' \
-s'twiggy top -n 20 ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly.html'
#-s'../binaryen/bin/wasm-opt -all -Oz -o ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ' \
#-s'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#buggy.
#-s'../binaryen/bin/wasm-opt -all -Oz -o ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ' \
#-s'ls -al ./webgl/target/wasm32-unknown-unknown/release/|grep makepad_webgl.webassembly' \
#-s'echo "Running Wasm opt..."'\
#-s'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#-s 'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' \
#cargo-watch watch -x 'build --target=wasm32-unknown-unknown --manifest-path="./webgl/Cargo.toml"' -s 'cp ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.wasm ./webgl/target/wasm32-unknown-unknown/release/makepad_webgl.webassembly' -s 'node ./build_index.js'
