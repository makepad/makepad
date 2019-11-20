// take a vec and return its innards as a [ptr,len,cap] triplet ptr
fn vec_to_js(vec: Vec<f32>) -> u32 {
    let vec = std::mem::ManuallyDrop::new(vec);
    let to_js = vec![
        vec.as_ptr() as u32,
        vec.len() as u32,
        vec.capacity() as u32
    ];
    let to_js = std::mem::ManuallyDrop::new(to_js);
    to_js.as_ptr() as u32
}

// free the triplet ptr returned by vec_to_js
#[export_name = "free_float_vec"]
pub unsafe extern "C"  fn free_float_vec(ptr: u32) {
    let to_js = Vec::<u32>::from_raw_parts(ptr as *mut u32, 3, 3);
    let _vec = Vec::<f32>::from_raw_parts(
        to_js[0] as *mut f32,
        to_js[1] as usize,
        to_js[2] as usize
    );
}

#[export_name = "hello_world"]
pub extern "C" fn hello_world(amount: u32) -> u32 {
    // this is the data we are returning
    let mut data = Vec::new();
    for i in 0..amount {
        data.push(i as f32)
    }
    vec_to_js(data)
}