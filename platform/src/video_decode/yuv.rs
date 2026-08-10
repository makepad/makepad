#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum YuvColorMatrix {
    BT709 = 0,
    BT601 = 1,
    BT2020 = 2,
}

impl YuvColorMatrix {
    pub fn as_f32(self) -> f32 {
        self as u8 as f32
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YuvLayout {
    I420,
    I422,
    I444,
    I400,
}

impl YuvLayout {
    pub fn chroma_size(self, luma_w: u32, luma_h: u32) -> (u32, u32) {
        match self {
            YuvLayout::I420 => (luma_w.div_ceil(2), luma_h.div_ceil(2)),
            YuvLayout::I422 => (luma_w.div_ceil(2), luma_h),
            YuvLayout::I444 => (luma_w, luma_h),
            YuvLayout::I400 => (0, 0),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct YuvPlaneData {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: YuvLayout,
    pub matrix: YuvColorMatrix,
    /// Row stride in bytes for Y. `0` means tightly packed (`== width`).
    pub y_stride: u32,
    /// Row stride in bytes for U. `0` means tightly packed (`== chroma_width`).
    pub u_stride: u32,
    /// Row stride in bytes for V. `0` means tightly packed (`== chroma_width`).
    pub v_stride: u32,
}

impl YuvPlaneData {
    pub fn y_bytes_per_row(&self) -> u32 {
        if self.y_stride == 0 {
            self.width
        } else {
            self.y_stride
        }
    }

    pub fn u_bytes_per_row(&self) -> u32 {
        let (cw, _) = self.layout.chroma_size(self.width, self.height);
        if self.u_stride == 0 {
            cw
        } else {
            self.u_stride
        }
    }

    pub fn v_bytes_per_row(&self) -> u32 {
        let (cw, _) = self.layout.chroma_size(self.width, self.height);
        if self.v_stride == 0 {
            cw
        } else {
            self.v_stride
        }
    }
}
