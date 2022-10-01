pub use ::makepad_error_log::{self,*};

fn main() {
    let mut data = Vec::new();
    data.resize(1024*1024*768, 0u8);
    let dt = std::time::Instant::now();
    let ret = makepad_miniz::base64_encode(&data, &makepad_miniz::BASE64_STANDARD);
    makepad_miniz::base64_decode(&ret).unwrap();
    log!("{}", dt.elapsed().as_millis());
    let dt = std::time::Instant::now(); 
    let b64 = base64::encode(&data);
    let out = base64::decode(&b64).unwrap();
    log!("{}", dt.elapsed().as_millis());
}