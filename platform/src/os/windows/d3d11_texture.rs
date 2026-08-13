//! Thin D3D11 texture helpers for cross-platform GPU texture code.
//!
//! COM method call sites live here so `windows_strip` (which scans
//! `os/windows/*.rs`) keeps the corresponding vendored methods.

use windows::{
    core::Result as WinResult,
    Win32::Graphics::Direct3D11::{
        ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11ShaderResourceView,
        ID3D11Texture2D, D3D11_BOX, D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SUBRESOURCE_DATA,
        D3D11_TEXTURE2D_DESC,
    },
};

pub unsafe fn texture2d_get_desc(texture: &ID3D11Texture2D, out: &mut D3D11_TEXTURE2D_DESC) {
    texture.GetDesc(out);
}

pub unsafe fn copy_subresource_region(
    context: &ID3D11DeviceContext,
    dst: &ID3D11Resource,
    dst_subresource: u32,
    dst_x: u32,
    dst_y: u32,
    dst_z: u32,
    src: &ID3D11Resource,
    src_subresource: u32,
    src_box: Option<*const D3D11_BOX>,
) {
    context.CopySubresourceRegion(
        dst,
        dst_subresource,
        dst_x,
        dst_y,
        dst_z,
        src,
        src_subresource,
        src_box,
    );
}

pub unsafe fn create_texture_2d(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
    initial_data: Option<*const D3D11_SUBRESOURCE_DATA>,
    texture_out: Option<*mut Option<ID3D11Texture2D>>,
) -> WinResult<()> {
    device.CreateTexture2D(desc, initial_data, texture_out)
}

pub unsafe fn create_shader_resource_view(
    device: &ID3D11Device,
    resource: &ID3D11Resource,
    desc: Option<*const D3D11_SHADER_RESOURCE_VIEW_DESC>,
    srv_out: Option<*mut Option<ID3D11ShaderResourceView>>,
) -> WinResult<()> {
    device.CreateShaderResourceView(resource, desc, srv_out)
}

pub unsafe fn device_get_immediate_context(
    device: &ID3D11Device,
) -> WinResult<ID3D11DeviceContext> {
    device.GetImmediateContext()
}
