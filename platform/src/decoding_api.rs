use crate::makepad_live_id::LiveId;

pub type VideoDecodingInputFn = Box<dyn FnMut(Vec<u8>) + Send  + 'static>;

pub trait CxDecodingApi {
    fn video_decoding_input<F>(&mut self, index: LiveId, f: F) where F: FnMut(Vec<u8>) + Send  + 'static{
        self.video_decoding_input_box(index, Box::new(f))
    }

    fn video_decoding_input_box(&mut self, index: LiveId, f: VideoDecodingInputFn);
} 
