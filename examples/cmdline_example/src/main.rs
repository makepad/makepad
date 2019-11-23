#[repr(transparent)]
pub struct CxUniforms{
    camera_projection:[f32;16],
    camera_view:[f32;16],
    dpi_factor:f32,
    dpi_dilate:f32
}

fn main(){
    println!("HELLO WORLD {}", std::mem::size_of::<CxUniforms>());
}
