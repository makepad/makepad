pub mod openxr {
    use crate::makepad_math::Mat4f;

    #[derive(Clone, Copy)]
    pub struct CxOpenXrEye {
        pub depth_view_mat: Mat4f,
        pub depth_proj_mat: Mat4f,
    }

    #[derive(Clone, Copy)]
    pub struct CxOpenXrFrame {
        pub eyes: [CxOpenXrEye; 1],
    }
}

pub mod vulkan {
    pub struct CxVulkan;

    pub struct CxVulkanOpenXrSessionData {
        pub depth_width: u32,
        pub depth_height: u32,
    }

    impl CxVulkan {
        pub fn read_openxr_depth_image(
            &mut self,
            _render_targets: &CxVulkanOpenXrSessionData,
            _depth_image_index: usize,
            _eye_index: usize,
        ) -> Result<Vec<u16>, String> {
            Err("OpenXR depth test stub does not provide image readback".to_string())
        }
    }
}

#[allow(dead_code, unused_imports, unused_variables)]
#[path = "linux/openxr_depth.rs"]
pub(crate) mod openxr_depth;
