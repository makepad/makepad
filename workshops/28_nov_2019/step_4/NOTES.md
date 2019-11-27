* In the file `/step_3/src/lib.rs`:

  * Replace the following line::

        extern "C" fn sierpinski(level: i32) {

  * With:

        extern "C" fn sierpinski(level: i32) -> i32 {

* In the file `/step_3/src/lib.rs`, in the function `sierpinski`, add the following line at the end:

      Box::into_raw(Box::new([1, 2, 3])) as i32

* In the file `/step_3/src/lib.rs`, add the following lines:

      #[no_mangle]
      pub unsafe extern "C" fn free_values(values: i32) {
          Box::from_raw(values as *mut [i32; 3]);
      }

* In the file `/step_3/static/wasm.js`, add the following lines:

      function sierpinski(level) {
        let valuesPtr = exports.sierpinski(level);
        let uint32Memory = new Uint32Array(exports.memory.buffer);
        let value_0 = uint32Memory[valuesPtr / 4 + 0];
        let value_1 = uint32Memory[valuesPtr / 4 + 1];
        let value_2 = uint32Memory[valuesPtr / 4 + 2];
        exports.free_values(valuesPtr);
        return [value_0, value_1, value_2];
      }

* In the file `/step_3/static/wasm.js`:

  * Replace the following line:
   
        return exports;

  * With:
  
        return { sierpinski };

* In the file `/step_3/static/main.js`:

  * Replace the following line:
   
        sierpinskiWasm(8);

  * With:
  
        console.log(sierpinskiWasm(8));