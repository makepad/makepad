* In `/step_4/src/lib.rs', add the following lines:
 
      pub unsafe extern "C" fn vec_f32_into_js(mut vec: Vec<f32>) -> i32 {
          let raw_parts = Box::new([
              vec.as_mut_ptr() as i32,
              vec.len() as i32,
              vec.capacity() as i32,
          ]);
          mem::forget(vec);
          Box::into_raw(raw_parts) as i32
      }

* In `/step_4/src/lib.rs`:

  * Replace the following line:
  
        Box::into_raw(Box::new([1, 2, 3])) as i32

  * With:
  
        return unsafe {
            vec_f32_into_js(vec![
                -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, -0.5, 0.5, 0.0, -0.5, 0.5, 0.0, 0.5, -0.5, 0.0, 0.5,
                0.5, 0.0,
            ])
        };

* In `/step_4/src/lib.rs`:

  * Replace the following lines:
        
        #[no_mangle]
        pub unsafe extern "C" fn free_values(values: i32) {
            Box::from_raw(values as *mut [i32; 3]);
        }

  * with:

        #[no_mangle]
        pub unsafe extern "C" fn free_vec_f32(raw_parts: i32) {
            let [ptr, length, capacity] = *Box::from_raw(raw_parts as *mut [i32; 3]);
            Vec::from_raw_parts(ptr as *mut f32, length as usize, capacity as usize);
        }

* In `/step_4/static/wasm.js`:

  * Replace the following lines:
    
        function sierpinski(level) {
          let valuesPtr = exports.sierpinski(level);
          let uint32Memory = new Uint32Array(exports.memory.buffer);
          let value_0 = uint32Memory[valuesPtr / 4 + 0];
          let value_1 = uint32Memory[valuesPtr / 4 + 1];
          let value_2 = uint32Memory[valuesPtr / 4 + 2];
          exports.free_values(valuesPtr);
          return [value_0, value_1, value_2];
        }

  * With:
    
        function sierpinski(level) {
          let rawPartsPtr = exports.sierpinski(level);
          let int32Memory = new Int32Array(exports.memory.buffer);
          let ptr = int32Memory[rawPartsPtr / 4 + 0];
          let len = int32Memory[rawPartsPtr / 4 + 1];
          let capacity = int32Memory[rawPartsPtr / 4 + 2];
          let float32Memory = new Float32Array(exports.memory.buffer);
          let result = float32Memory.subarray(ptr / 4, ptr / 4 + len).slice();
          exports.free_vec_f32(rawPartsPtr);
          return result;
        }