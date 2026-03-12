use {
    crate::{
        cx::Cx, id_pool::*, makepad_error_log::*, makepad_script::*, os::CxOsUniformBuffer,
        script::vm::*,
    },
    std::rc::Rc,
};

#[derive(Debug, Clone, PartialEq)]
pub struct UniformBuffer(Rc<PoolId>);

#[derive(Clone, Debug, PartialEq, Copy)]
pub struct UniformBufferId(pub(crate) usize, u64);

impl UniformBuffer {
    pub fn new(cx: &mut Cx) -> Self {
        cx.uniform_buffers.alloc()
    }

    pub fn uniform_buffer_id(&self) -> UniformBufferId {
        UniformBufferId(self.0.id, self.0.generation)
    }

    pub fn clear(&self, cx: &mut Cx) {
        cx.uniform_buffers[self.uniform_buffer_id()].data.clear();
    }

    pub fn set_bytes(&self, cx: &mut Cx, data: &[u8]) {
        let cx_uniform_buffer = &mut cx.uniform_buffers[self.uniform_buffer_id()];
        cx_uniform_buffer.data.clear();
        cx_uniform_buffer.data.extend_from_slice(data);
    }

    pub fn set_struct<T: Copy>(&self, cx: &mut Cx, value: &T) {
        let bytes = unsafe {
            std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>())
        };
        self.set_bytes(cx, bytes);
    }

    pub fn set_struct_slice<T: Copy>(&self, cx: &mut Cx, values: &[T]) {
        let bytes = unsafe {
            std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
        };
        self.set_bytes(cx, bytes);
    }
}

impl ScriptHook for UniformBuffer {}
impl ScriptApply for UniformBuffer {}
impl ScriptNew for UniformBuffer {
    fn script_new(vm: &mut ScriptVm) -> Self {
        Self::new(vm.cx_mut())
    }
}

#[derive(Default)]
pub struct CxUniformBufferPool(pub(crate) IdPool<CxUniformBuffer>);

impl CxUniformBufferPool {
    pub fn alloc(&mut self) -> UniformBuffer {
        let (new_id, previous_item) = self
            .0
            .alloc_with_reuse_filter(|_| true, CxUniformBuffer::default());
        if let Some(previous_item) = previous_item {
            self.0.pool[new_id.id].item.os = previous_item.os;
        }
        UniformBuffer(Rc::new(new_id))
    }
}

impl std::ops::Index<UniformBufferId> for CxUniformBufferPool {
    type Output = CxUniformBuffer;
    fn index(&self, index: UniformBufferId) -> &Self::Output {
        let d = &self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "UniformBuffer id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &d.item
    }
}

impl std::ops::IndexMut<UniformBufferId> for CxUniformBufferPool {
    fn index_mut(&mut self, index: UniformBufferId) -> &mut Self::Output {
        let d = &mut self.0.pool[index.0];
        if d.generation != index.1 {
            error!(
                "UniformBuffer id generation wrong {} {} {}",
                index.0, d.generation, index.1
            )
        }
        &mut d.item
    }
}

#[derive(Default)]
pub struct CxUniformBuffer {
    pub data: Vec<u8>,
    pub os: CxOsUniformBuffer,
}
