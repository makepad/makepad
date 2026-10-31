//! Device buffers, reached by kernels through their device address.
//!
//! On unified memory (phones) every buffer is device memory the CPU maps
//! directly: uploads and readbacks are plain copies. On a discrete GPU
//! device-only buffers are filled and read through a staging buffer.

use crate::device::{vk_err, Device, Result};
use makepad_vulkan::vk;

pub struct Buffer {
    pub(crate) buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub size: u64,
    /// The device address kernels read it at.
    pub address: u64,
    mapped: *mut u8,
    device: makepad_vulkan::Device,
}

// The mapping is plain memory owned by this buffer.
unsafe impl Send for Buffer {}
unsafe impl Sync for Buffer {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BufferKind {
    /// Kernel data; host-mapped when the device memory is unified.
    Device,
    /// Host-mapped staging memory (cached for readbacks).
    Staging,
}

impl Buffer {
    /// The mapped bytes (unified device memory and staging buffers).
    pub fn mapped(&self) -> Option<*mut u8> {
        if self.mapped.is_null() {
            None
        } else {
            Some(self.mapped)
        }
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        unsafe {
            if !self.mapped.is_null() {
                self.device.unmap_memory(self.memory);
            }
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

impl Device {
    /// A buffer of `size` bytes with a device address.
    pub fn buffer(&self, size: u64, kind: BufferKind) -> Result<Buffer> {
        unsafe { self.create_buffer(size.max(16), kind) }
    }

    unsafe fn create_buffer(&self, size: u64, kind: BufferKind) -> Result<Buffer> {
        let usage = vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
            | vk::BufferUsageFlags::TRANSFER_SRC
            | vk::BufferUsageFlags::TRANSFER_DST;
        let buffer = self
            .device
            .create_buffer(&vk::BufferCreateInfo::default().size(size).usage(usage), None)
            .map_err(vk_err("vkCreateBuffer"))?;
        let req = self.device.get_buffer_memory_requirements(buffer);
        let hv = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        let dl = vk::MemoryPropertyFlags::DEVICE_LOCAL;
        let (type_index, map) = match kind {
            BufferKind::Device if self.features.unified_memory => {
                (self.memory_type(req.memory_type_bits, dl | hv), true)
            }
            BufferKind::Device => (self.memory_type(req.memory_type_bits, dl), false),
            BufferKind::Staging => (
                self.memory_type(req.memory_type_bits, hv | vk::MemoryPropertyFlags::HOST_CACHED)
                    .or_else(|| self.memory_type(req.memory_type_bits, hv)),
                true,
            ),
        };
        let Some(type_index) = type_index else {
            self.device.destroy_buffer(buffer, None);
            return Err(format!("no memory type for a {kind:?} buffer"));
        };
        let mut flags = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let info = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(type_index)
            .next_memory_allocate_flags_info(&mut flags);
        let memory = match self.device.allocate_memory(&info, None) {
            Ok(m) => m,
            Err(e) => {
                self.device.destroy_buffer(buffer, None);
                return Err(format!("vkAllocateMemory {} bytes: {e:?}", req.size));
            }
        };
        self.device.bind_buffer_memory(buffer, memory, 0).map_err(vk_err("vkBindBufferMemory"))?;
        let address = self.device.get_buffer_device_address(&vk::BufferDeviceAddressInfo::default().buffer(buffer));
        let mapped = if map {
            self.device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .map_err(vk_err("vkMapMemory"))? as *mut u8
        } else {
            std::ptr::null_mut()
        };
        Ok(Buffer { buffer, memory, size, address, mapped, device: self.device.clone() })
    }
}
