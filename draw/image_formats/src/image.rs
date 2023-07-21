// image_formats::image
// by Desmond Germans, 2019

#[derive(Default, Clone)] 
pub struct ImageBuffer {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

impl ImageBuffer {
    pub fn new(width: usize,height: usize) -> ImageBuffer {
        ImageBuffer {
            width,
            height,
            data: vec![0; width * height],
        }
    }
}
