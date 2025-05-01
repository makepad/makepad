use makepad_micro_serde::*;
use makepad_widgets::*;



#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUINodeError {
    pub unknown: Option<String>
}


#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIResponse {
    pub prompt_id: String,
    pub number: String,
    pub node_errors: ComfyUINodeError,
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIMessage {
    pub _type: String,
    pub data: ComfyUIData
}
#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIData {
    pub value: Option<u32>,
    pub max: Option<u32>,
    pub node: Option<String>,
    pub prompt_id: Option<String>,
    pub nodes: Option<Vec<String >>,
    pub display_node: Option<String>,
    pub timestamp: Option<u64>,
    pub status: Option<ComfyUIStatus>,
    pub sid: Option<String>,
    pub output: Option<ComfyUIOutput>
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIStatus {
    pub exec_info: ComfyUIExecInfo,
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIOutput {
    pub images: Vec<ComfyUIImage>,
    pub animated: Option<Vec<bool>>
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIImage {
    pub filename: String,
    pub subfolder: String,
    pub _type: String
}

#[allow(dead_code)]
#[derive(DeJson, Debug)]
pub struct ComfyUIExecInfo {
    pub queue_remaining: u32
}

