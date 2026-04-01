use std::collections::HashMap;

use crate::core::{ggml_pad, InitParams, GGML_MEM_ALIGN, GGML_MROPE_SECTIONS};
use crate::op::{GluOp, Op, Prec, UnaryOp};
use crate::tensor::{
    ggml_type_size_for_type, BufferUsage, Tensor, TensorDesc, TensorId, TensorLayout, TensorType,
};

#[derive(Clone, Debug)]
pub struct Context {
    mem_size: usize,
    mem_buffer: Vec<u8>,
    no_alloc: bool,
    next_data_offset: usize,
    max_tensor_size: usize,
    tensors: Vec<Tensor>,
    name_to_tensor: HashMap<String, TensorId>,
}

impl Context {
    pub fn new(params: InitParams) -> Self {
        let mem_buffer = match params.mem_buffer {
            Some(buffer) => buffer,
            None if params.mem_size > 0 => vec![0; params.mem_size],
            None => Vec::new(),
        };

        let mem_size = params.mem_size.max(mem_buffer.len());

        Self {
            mem_size,
            mem_buffer,
            no_alloc: params.no_alloc,
            next_data_offset: 0,
            max_tensor_size: 0,
            tensors: Vec::new(),
            name_to_tensor: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.next_data_offset = 0;
        self.max_tensor_size = 0;
        self.tensors.clear();
        self.name_to_tensor.clear();
    }

    pub fn used_mem(&self) -> usize {
        self.next_data_offset
    }

    pub fn get_no_alloc(&self) -> bool {
        self.no_alloc
    }

    pub fn set_no_alloc(&mut self, no_alloc: bool) {
        self.no_alloc = no_alloc;
    }

    pub fn mem_buffer(&self) -> &[u8] {
        &self.mem_buffer
    }

    pub fn mem_buffer_mut(&mut self) -> &mut [u8] {
        &mut self.mem_buffer
    }

    pub fn mem_size(&self) -> usize {
        self.mem_size
    }

    pub fn max_tensor_size(&self) -> usize {
        self.max_tensor_size
    }

    pub fn tensors(&self) -> &[Tensor] {
        &self.tensors
    }

    pub fn tensor(&self, id: TensorId) -> Option<&Tensor> {
        self.tensors.get(id)
    }

    pub fn tensor_mut(&mut self, id: TensorId) -> Option<&mut Tensor> {
        self.tensors.get_mut(id)
    }

    pub fn get_tensor(&self, name: &str) -> Option<TensorId> {
        self.name_to_tensor.get(name).copied()
    }

    pub fn new_buffer(&mut self, nbytes: usize) -> Result<usize, String> {
        let offset = ggml_pad(self.next_data_offset, GGML_MEM_ALIGN);
        let end = offset
            .checked_add(nbytes)
            .ok_or_else(|| "buffer allocation overflow".to_string())?;
        if end > self.mem_size {
            return Err(format!(
                "context buffer too small: need {} bytes, have {} bytes",
                end, self.mem_size
            ));
        }
        self.next_data_offset = end;
        Ok(offset)
    }

    pub fn new_tensor(
        &mut self,
        ty: TensorType,
        n_dims: usize,
        ne: &[i64],
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        if n_dims == 0 || n_dims > 4 {
            return Err(format!("invalid tensor rank {}", n_dims));
        }
        if ne.len() != n_dims {
            return Err(format!(
                "rank {} but got {} extents",
                n_dims,
                ne.len()
            ));
        }

        let layout = TensorLayout::for_ggml(ty, ne)?;
        let desc = TensorDesc::new(ty, layout, usage);
        self.push_tensor(Tensor::from_desc(self.tensors.len(), desc), true)
    }

    pub fn new_named_tensor(
        &mut self,
        name: impl Into<String>,
        ty: TensorType,
        n_dims: usize,
        ne: &[i64],
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        if n_dims == 0 || n_dims > 4 {
            return Err(format!("invalid tensor rank {}", n_dims));
        }
        if ne.len() != n_dims {
            return Err(format!("rank {} but got {} extents", n_dims, ne.len()));
        }

        let layout = TensorLayout::for_ggml(ty, ne)?;
        let desc = TensorDesc::new(ty, layout, usage).with_name(name);
        self.push_tensor(Tensor::from_desc(self.tensors.len(), desc), true)
    }

    pub fn new_tensor_1d(
        &mut self,
        ty: TensorType,
        ne0: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.new_tensor(ty, 1, &[ne0], usage)
    }

    pub fn new_tensor_2d(
        &mut self,
        ty: TensorType,
        ne0: i64,
        ne1: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.new_tensor(ty, 2, &[ne0, ne1], usage)
    }

    pub fn new_tensor_3d(
        &mut self,
        ty: TensorType,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.new_tensor(ty, 3, &[ne0, ne1, ne2], usage)
    }

    pub fn new_tensor_4d(
        &mut self,
        ty: TensorType,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        ne3: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.new_tensor(ty, 4, &[ne0, ne1, ne2, ne3], usage)
    }

    pub fn dup_tensor(&mut self, src: TensorId) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let desc = TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), tensor.desc.usage);
        self.push_tensor(Tensor::from_desc(self.tensors.len(), desc), true)
    }

    pub fn view_tensor(&mut self, src: TensorId) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let mut view = Tensor::from_desc(
            self.tensors.len(),
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), tensor.desc.usage),
        );
        view.view_src = Some(src);
        view.view_offs = 0;
        view.buffer_id = tensor.buffer_id;
        view.data_offset = tensor.data_offset;
        self.push_tensor(view, false)
    }

    pub fn set_tensor_name(
        &mut self,
        id: TensorId,
        name: impl Into<String>,
    ) -> Result<(), String> {
        let existing = self
            .tensor(id)
            .ok_or_else(|| format!("invalid tensor id {}", id))?
            .name()
            .map(str::to_owned);
        if let Some(existing) = existing {
            self.name_to_tensor.remove(&existing);
        }
        let new_name = {
            let tensor = self
                .tensor_mut(id)
                .ok_or_else(|| format!("invalid tensor id {}", id))?;
            tensor.set_name(name.into());
            tensor.name().map(str::to_owned)
        };
        if let Some(name) = new_name {
            self.name_to_tensor.insert(name, id);
        }
        Ok(())
    }

    pub fn set_tensor_layout(
        &mut self,
        id: TensorId,
        layout: TensorLayout,
    ) -> Result<(), String> {
        let tensor = self
            .tensor_mut(id)
            .ok_or_else(|| format!("invalid tensor id {}", id))?;
        tensor.ne = layout.extents4();
        tensor.nb = layout.strides_bytes();
        tensor.desc.layout = layout;
        Ok(())
    }

    pub fn tensor_data(&self, id: TensorId) -> Result<&[u8], String> {
        let tensor = self
            .tensor(id)
            .ok_or_else(|| format!("invalid tensor id {}", id))?;
        let offset = tensor
            .data_offset
            .ok_or_else(|| format!("tensor {} has no allocated data offset", id))?;
        let end = offset
            .checked_add(tensor.nbytes())
            .ok_or_else(|| format!("tensor {} byte range overflow", id))?;
        self.mem_buffer
            .get(offset..end)
            .ok_or_else(|| format!("tensor {} byte range [{}..{}) is out of bounds", id, offset, end))
    }

    pub fn tensor_data_mut(&mut self, id: TensorId) -> Result<&mut [u8], String> {
        let tensor = self
            .tensor(id)
            .ok_or_else(|| format!("invalid tensor id {}", id))?;
        let offset = tensor
            .data_offset
            .ok_or_else(|| format!("tensor {} has no allocated data offset", id))?;
        let end = offset
            .checked_add(tensor.nbytes())
            .ok_or_else(|| format!("tensor {} byte range overflow", id))?;
        self.mem_buffer
            .get_mut(offset..end)
            .ok_or_else(|| format!("tensor {} byte range [{}..{}) is out of bounds", id, offset, end))
    }

    pub fn write_tensor_data(&mut self, id: TensorId, bytes: &[u8]) -> Result<(), String> {
        let dst = self.tensor_data_mut(id)?;
        if dst.len() != bytes.len() {
            return Err(format!(
                "tensor {} byte length mismatch: dst={} src={}",
                id,
                dst.len(),
                bytes.len()
            ));
        }
        dst.copy_from_slice(bytes);
        Ok(())
    }

    pub fn view(
        &mut self,
        src: TensorId,
        ty: TensorType,
        extents: &[i64],
        strides_bytes: &[usize],
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        let usage = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .usage;
        let layout = TensorLayout::from_parts(extents.len(), extents, strides_bytes)?;
        let mut tensor = Tensor::from_desc(
            self.tensors.len(),
            TensorDesc::new(ty, layout, usage),
        );
        tensor.op = Op::View;
        tensor.view_src = Some(src);
        tensor.view_offs = offset_bytes;
        tensor.src[0] = Some(src);
        self.push_tensor(tensor, false)
    }

    pub fn view_1d(
        &mut self,
        src: TensorId,
        ne0: i64,
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        let ty = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .ty;
        self.view(
            src,
            ty,
            &[ne0],
            &[ggml_type_size_for_type(ty)],
            offset_bytes,
        )
    }

    pub fn view_2d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        nb1: usize,
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        let ty = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .ty;
        self.view(
            src,
            ty,
            &[ne0, ne1],
            &[ggml_type_size_for_type(ty), nb1],
            offset_bytes,
        )
    }

    pub fn view_3d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        nb1: usize,
        nb2: usize,
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        let ty = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .ty;
        self.view(
            src,
            ty,
            &[ne0, ne1, ne2],
            &[ggml_type_size_for_type(ty), nb1, nb2],
            offset_bytes,
        )
    }

    pub fn view_4d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        ne3: i64,
        nb1: usize,
        nb2: usize,
        nb3: usize,
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        let ty = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .ty;
        self.view(
            src,
            ty,
            &[ne0, ne1, ne2, ne3],
            &[ggml_type_size_for_type(ty), nb1, nb2, nb3],
            offset_bytes,
        )
    }

    pub fn new_view_op_tensor(
        &mut self,
        desc: TensorDesc,
        op: Op,
        src: TensorId,
        offset_bytes: usize,
    ) -> Result<TensorId, String> {
        self.new_view_op_tensor_with_srcs(desc, op, src, offset_bytes, &[src])
    }

    pub fn new_view_op_tensor_with_srcs(
        &mut self,
        desc: TensorDesc,
        op: Op,
        view_src: TensorId,
        offset_bytes: usize,
        srcs: &[TensorId],
    ) -> Result<TensorId, String> {
        let source = self
            .tensor(view_src)
            .ok_or_else(|| format!("invalid tensor id {}", view_src))?;
        let mut tensor = Tensor::from_desc(self.tensors.len(), desc);
        tensor.op = op;
        tensor.view_src = Some(view_src);
        tensor.view_offs = offset_bytes;
        tensor.buffer_id = source.buffer_id;
        tensor.data_offset = source.data_offset;
        for (i, src) in srcs.iter().copied().enumerate() {
            if i >= tensor.src.len() {
                return Err(format!("too many op sources: {}", srcs.len()));
            }
            tensor.src[i] = Some(src);
        }
        self.push_tensor(tensor, false)
    }

    pub fn new_op_tensor(
        &mut self,
        desc: TensorDesc,
        op: Op,
        srcs: &[TensorId],
    ) -> Result<TensorId, String> {
        let mut tensor = Tensor::from_desc(self.tensors.len(), desc);
        tensor.op = op;
        for (i, src) in srcs.iter().copied().enumerate() {
            if i >= tensor.src.len() {
                return Err(format!("too many op sources: {}", srcs.len()));
            }
            tensor.src[i] = Some(src);
        }
        self.push_tensor(tensor, true)
    }

    pub fn unary(
        &mut self,
        a: TensorId,
        unary: UnaryOp,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let src = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        let id = self.new_op_tensor(
            TensorDesc::new(src.desc.ty, src.desc.layout.clone(), usage),
            Op::Unary,
            &[a],
        )?;
        self.tensor_mut(id).unwrap().set_unary_op(unary);
        Ok(id)
    }

    pub fn clamp(
        &mut self,
        src: TensorId,
        min: f32,
        max: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let id = self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::Clamp,
            &[src],
        )?;
        let tensor = self.tensor_mut(id).unwrap();
        tensor.set_op_param_f32(4, min);
        tensor.set_op_param_f32(5, max);
        Ok(id)
    }

    pub fn scale(
        &mut self,
        src: TensorId,
        scale: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let id = self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::Scale,
            &[src],
        )?;
        self.tensor_mut(id).unwrap().set_op_param_f32(1, scale);
        Ok(id)
    }

    pub fn glu(
        &mut self,
        a: TensorId,
        glu: GluOp,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let src = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        let id = self.new_op_tensor(
            TensorDesc::new(src.desc.ty, src.desc.layout.clone(), usage),
            Op::Glu,
            &[a],
        )?;
        self.tensor_mut(id).unwrap().set_glu_op(glu);
        Ok(id)
    }

    pub fn binary_like_a(
        &mut self,
        op: Op,
        a: TensorId,
        b: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let src = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        self.new_op_tensor(
            TensorDesc::new(src.desc.ty, src.desc.layout.clone(), usage),
            op,
            &[a, b],
        )
    }

    pub fn mul_mat(
        &mut self,
        a: TensorId,
        b: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let ta = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        let tb = self
            .tensor(b)
            .ok_or_else(|| format!("invalid tensor id {}", b))?;
        let ne = [ta.ne[1], tb.ne[1], tb.ne[2], tb.ne[3]];
        let layout = TensorLayout::for_ggml(TensorType::F32, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::F32, layout, usage),
            Op::MulMat,
            &[a, b],
        )
    }

    pub fn mul_mat_id(
        &mut self,
        as_tensor: TensorId,
        b: TensorId,
        ids: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let as_ref = self
            .tensor(as_tensor)
            .ok_or_else(|| format!("invalid tensor id {}", as_tensor))?;
        let b_ref = self
            .tensor(b)
            .ok_or_else(|| format!("invalid tensor id {}", b))?;
        let ids_ref = self
            .tensor(ids)
            .ok_or_else(|| format!("invalid tensor id {}", ids))?;

        if ids_ref.desc.ty != TensorType::I32 {
            return Err(format!(
                "mul_mat_id ids tensor must be I32, got {}",
                ids_ref.desc.ty.name()
            ));
        }
        if as_ref.is_transposed() {
            return Err("mul_mat_id expert tensor must not be transposed".to_string());
        }
        if b_ref.is_transposed() {
            return Err("mul_mat_id input tensor must not be transposed".to_string());
        }
        if as_ref.ne[3] != 1 {
            return Err(format!(
                "mul_mat_id expert tensor must be 3D, got ne3={}",
                as_ref.ne[3]
            ));
        }
        if b_ref.ne[3] != 1 {
            return Err(format!(
                "mul_mat_id input tensor must be 3D, got ne3={}",
                b_ref.ne[3]
            ));
        }
        if ids_ref.ne[2] != 1 || ids_ref.ne[3] != 1 {
            return Err(format!(
                "mul_mat_id ids tensor must be 2D, got ne2={} ne3={}",
                ids_ref.ne[2], ids_ref.ne[3]
            ));
        }
        if ids_ref.ne[1] != b_ref.ne[2] {
            return Err(format!(
                "mul_mat_id ids rows {} do not match input tokens {}",
                ids_ref.ne[1], b_ref.ne[2]
            ));
        }
        if as_ref.ne[0] != b_ref.ne[0] {
            return Err(format!(
                "mul_mat_id source width mismatch: experts={} input={}",
                as_ref.ne[0], b_ref.ne[0]
            ));
        }
        if ids_ref.ne[0] % b_ref.ne[1] != 0 {
            return Err(format!(
                "mul_mat_id ids expert count {} is not broadcast-compatible with input experts {}",
                ids_ref.ne[0], b_ref.ne[1]
            ));
        }

        let layout = TensorLayout::for_ggml(
            TensorType::F32,
            &[as_ref.ne[1], ids_ref.ne[0], b_ref.ne[2], 1],
        )?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::F32, layout, usage),
            Op::MulMatId,
            &[as_tensor, b, ids],
        )
    }

    pub fn add_id(
        &mut self,
        a: TensorId,
        b: TensorId,
        ids: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let a_ref = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        let b_ref = self
            .tensor(b)
            .ok_or_else(|| format!("invalid tensor id {}", b))?;
        let ids_ref = self
            .tensor(ids)
            .ok_or_else(|| format!("invalid tensor id {}", ids))?;

        if ids_ref.desc.ty != TensorType::I32 {
            return Err(format!(
                "add_id ids tensor must be I32, got {}",
                ids_ref.desc.ty.name()
            ));
        }
        if a_ref.ne[0] != b_ref.ne[0] {
            return Err(format!(
                "add_id width mismatch: lhs={} rhs={}",
                a_ref.ne[0], b_ref.ne[0]
            ));
        }
        if a_ref.ne[1] != ids_ref.ne[0] {
            return Err(format!(
                "add_id lhs dim1 {} does not match ids dim0 {}",
                a_ref.ne[1], ids_ref.ne[0]
            ));
        }
        if a_ref.ne[2] != ids_ref.ne[1] {
            return Err(format!(
                "add_id lhs dim2 {} does not match ids dim1 {}",
                a_ref.ne[2], ids_ref.ne[1]
            ));
        }

        self.new_op_tensor(
            TensorDesc::new(a_ref.desc.ty, a_ref.desc.layout.clone(), usage),
            Op::AddId,
            &[a, b, ids],
        )
    }

    pub fn concat(
        &mut self,
        a: TensorId,
        b: TensorId,
        dim: usize,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let ta = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        let tb = self
            .tensor(b)
            .ok_or_else(|| format!("invalid tensor id {}", b))?;
        let rank = ta.desc.layout.rank().max(tb.desc.layout.rank());
        if dim >= 4 {
            return Err(format!("concat dimension {} exceeds ggml rank 4", dim));
        }
        if dim >= rank {
            return Err(format!(
                "concat dimension {} exceeds tensor rank {}",
                dim, rank
            ));
        }
        if ta.desc.ty != tb.desc.ty {
            return Err(format!(
                "concat requires matching types, got {} and {}",
                ta.desc.ty.name(),
                tb.desc.ty.name()
            ));
        }
        let mut ne = ta.ne;
        for (idx, (&ea, &eb)) in ta.ne[..rank].iter().zip(tb.ne[..rank].iter()).enumerate() {
            if idx == dim {
                ne[idx] = ea
                    .checked_add(eb)
                    .ok_or_else(|| "concat extent overflow".to_string())?;
            } else if ea != eb {
                return Err(format!(
                    "concat requires matching extent at dim {}: {} vs {}",
                    idx, ea, eb
                ));
            }
        }
        let layout = TensorLayout::for_ggml(ta.desc.ty, &ne[..rank])?;
        let id = self.new_op_tensor(
            TensorDesc::new(ta.desc.ty, layout, usage),
            Op::Concat,
            &[a, b],
        )?;
        self.tensor_mut(id).unwrap().set_op_param_i32(0, dim as i32);
        Ok(id)
    }

    pub fn reshape(
        &mut self,
        src: TensorId,
        extents: &[i64],
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let layout = TensorLayout::for_ggml(tensor.desc.ty, extents)?;
        self.new_view_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, tensor.desc.usage),
            Op::Reshape,
            src,
            0,
        )
    }

    pub fn cpy(
        &mut self,
        src: TensorId,
        dst: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let src_tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let dst_tensor = self
            .tensor(dst)
            .ok_or_else(|| format!("invalid tensor id {}", dst))?;
        if src_tensor.nelements() != dst_tensor.nelements() {
            return Err(format!(
                "cpy requires matching element counts, got {} and {}",
                src_tensor.nelements(),
                dst_tensor.nelements()
            ));
        }
        let desc = TensorDesc::new(dst_tensor.desc.ty, dst_tensor.desc.layout.clone(), usage);
        self.new_view_op_tensor_with_srcs(desc, Op::Cpy, dst, 0, &[src, dst])
    }

    pub fn cont(&mut self, src: TensorId) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(
                tensor.desc.ty,
                TensorLayout::for_ggml(tensor.desc.ty, tensor.desc.layout.extents())?,
                tensor.desc.usage,
            ),
            Op::Cont,
            &[src],
        )
    }

    pub fn cont_2d(&mut self, src: TensorId, ne0: i64, ne1: i64) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(
                tensor.desc.ty,
                TensorLayout::for_ggml(tensor.desc.ty, &[ne0, ne1])?,
                tensor.desc.usage,
            ),
            Op::Cont,
            &[src],
        )
    }

    pub fn cont_3d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        ne2: i64,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(
                tensor.desc.ty,
                TensorLayout::for_ggml(tensor.desc.ty, &[ne0, ne1, ne2])?,
                tensor.desc.usage,
            ),
            Op::Cont,
            &[src],
        )
    }

    pub fn cont_4d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        ne3: i64,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(
                tensor.desc.ty,
                TensorLayout::for_ggml(tensor.desc.ty, &[ne0, ne1, ne2, ne3])?,
                tensor.desc.usage,
            ),
            Op::Cont,
            &[src],
        )
    }

    pub fn permute(&mut self, src: TensorId, axes: [usize; 4]) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let rank = tensor.desc.layout.rank();
        let mut extents = Vec::with_capacity(rank);
        let mut strides = Vec::with_capacity(rank);
        for &src_idx in axes.iter().take(rank) {
            if src_idx >= rank {
                return Err(format!(
                    "permute axis {} exceeds tensor '{}' rank {}",
                    src_idx,
                    tensor.name().unwrap_or("<unnamed>"),
                    rank
                ));
            }
            extents.push(tensor.ne[src_idx]);
            strides.push(tensor.nb[src_idx]);
        }
        let layout = TensorLayout::from_parts(rank, &extents, &strides)?;
        self.new_view_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, tensor.desc.usage),
            Op::Permute,
            src,
            0,
        )
    }

    pub fn transpose(&mut self, src: TensorId) -> Result<TensorId, String> {
        self.permute(src, [1, 0, 2, 3]).map(|id| {
            let tensor = self.tensor_mut(id).unwrap();
            tensor.op = Op::Transpose;
            id
        })
    }

    pub fn repeat(
        &mut self,
        src: TensorId,
        shape_of: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let target = self
            .tensor(shape_of)
            .ok_or_else(|| format!("invalid tensor id {}", shape_of))?;
        self.new_op_tensor(
            TensorDesc::new(target.desc.ty, target.desc.layout.clone(), usage),
            Op::Repeat,
            &[src, shape_of],
        )
    }

    pub fn repeat_4d(
        &mut self,
        src: TensorId,
        ne0: i64,
        ne1: i64,
        ne2: i64,
        ne3: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let ty = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?
            .desc
            .ty;
        let layout = TensorLayout::for_ggml(ty, &[ne0, ne1, ne2, ne3])?;
        self.new_op_tensor(
            TensorDesc::new(ty, layout, usage),
            Op::Repeat,
            &[src],
        )
    }

    pub fn sum_rows(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let ne = [1, tensor.ne[1], tensor.ne[2], tensor.ne[3]];
        let layout = TensorLayout::for_ggml(tensor.desc.ty, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, usage),
            Op::SumRows,
            &[src],
        )
    }

    pub fn cumsum(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::CumSum,
            &[src],
        )
    }

    pub fn diag(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::Diag,
            &[src],
        )
    }

    pub fn tri(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::Tri,
            &[src],
        )
    }

    pub fn solve_tri(
        &mut self,
        a: TensorId,
        b: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.binary_like_a(Op::SolveTri, a, b, usage)
    }

    pub fn get_rows(
        &mut self,
        src: TensorId,
        rows: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let rows_tensor = self
            .tensor(rows)
            .ok_or_else(|| format!("invalid tensor id {}", rows))?;
        let ne = [tensor.ne[0], rows_tensor.ne[0], rows_tensor.ne[1], rows_tensor.ne[2]];
        let ty = if tensor.desc.ty == TensorType::I32 {
            TensorType::I32
        } else {
            TensorType::F32
        };
        let layout = TensorLayout::for_ggml(ty, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(ty, layout, usage),
            Op::GetRows,
            &[src, rows],
        )
    }

    pub fn set_rows(
        &mut self,
        dst: TensorId,
        src: TensorId,
        rows: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let dst_tensor = self
            .tensor(dst)
            .ok_or_else(|| format!("invalid tensor id {}", dst))?;
        let src_tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let rows_tensor = self
            .tensor(rows)
            .ok_or_else(|| format!("invalid tensor id {}", rows))?;
        if dst_tensor.ne[0] != src_tensor.ne[0] {
            return Err(format!(
                "set_rows requires dst dim0 {} to match src dim0 {}",
                dst_tensor.ne[0], src_tensor.ne[0]
            ));
        }
        if dst_tensor.ne[2] != src_tensor.ne[2] || dst_tensor.ne[3] != src_tensor.ne[3] {
            return Err(format!(
                "set_rows requires dst dims2/3 [{}, {}] to match src dims2/3 [{}, {}]",
                dst_tensor.ne[2], dst_tensor.ne[3], src_tensor.ne[2], src_tensor.ne[3]
            ));
        }
        if src_tensor.ne[1] != rows_tensor.ne[0] {
            return Err(format!(
                "set_rows requires src dim1 {} to match rows dim0 {}",
                src_tensor.ne[1], rows_tensor.ne[0]
            ));
        }
        if rows_tensor.desc.ty != TensorType::I32 && rows_tensor.desc.ty != TensorType::I64 {
            return Err(format!(
                "set_rows rows tensor must be I32 or I64, got {}",
                rows_tensor.desc.ty.name()
            ));
        }
        if src_tensor.desc.ty != TensorType::F32 {
            return Err(format!(
                "set_rows currently requires F32 src rows, got {}",
                src_tensor.desc.ty.name()
            ));
        }
        let desc = TensorDesc::new(dst_tensor.desc.ty, dst_tensor.desc.layout.clone(), usage);
        self.new_view_op_tensor_with_srcs(desc, Op::SetRows, dst, 0, &[src, rows, dst])
    }

    pub fn soft_max(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let id = self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::SoftMax,
            &[src],
        )?;
        let tensor = self.tensor_mut(id).unwrap();
        tensor.set_op_param_f32(0, 1.0);
        tensor.set_op_param_f32(1, 0.0);
        Ok(id)
    }

    pub fn flash_attn_ext(
        &mut self,
        q: TensorId,
        k: TensorId,
        v: TensorId,
        mask: Option<TensorId>,
        scale: f32,
        max_bias: f32,
        logit_softcap: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let q_tensor = self
            .tensor(q)
            .ok_or_else(|| format!("invalid tensor id {}", q))?;
        let k_tensor = self
            .tensor(k)
            .ok_or_else(|| format!("invalid tensor id {}", k))?;
        let v_tensor = self
            .tensor(v)
            .ok_or_else(|| format!("invalid tensor id {}", v))?;

        if q_tensor.ne[3] != k_tensor.ne[3] || q_tensor.ne[3] != v_tensor.ne[3] {
            return Err("flash_attn_ext requires q/k/v to agree on dim3".to_string());
        }

        if let Some(mask) = mask {
            let mask_tensor = self
                .tensor(mask)
                .ok_or_else(|| format!("invalid tensor id {}", mask))?;
            if mask_tensor.desc.ty != TensorType::F16 {
                return Err(format!(
                    "flash_attn_ext mask must be F16, got {}",
                    mask_tensor.desc.ty.name()
                ));
            }
        }

        let ne = [v_tensor.ne[0], q_tensor.ne[2], q_tensor.ne[1], q_tensor.ne[3]];
        let layout = TensorLayout::for_ggml(TensorType::F32, &ne)?;
        let mut srcs = vec![q, k, v];
        if let Some(mask) = mask {
            srcs.push(mask);
        }

        let id = self.new_op_tensor(
            TensorDesc::new(TensorType::F32, layout, usage),
            Op::FlashAttnExt,
            &srcs,
        )?;
        let tensor = self.tensor_mut(id).unwrap();
        tensor.set_op_param_f32(0, scale);
        tensor.set_op_param_f32(1, max_bias);
        tensor.set_op_param_f32(2, logit_softcap);
        tensor.set_op_param_i32(3, Prec::Default as i32);
        Ok(id)
    }

    pub fn flash_attn_ext_set_prec(
        &mut self,
        tensor_id: TensorId,
        prec: Prec,
    ) -> Result<(), String> {
        let tensor = self
            .tensor_mut(tensor_id)
            .ok_or_else(|| format!("invalid tensor id {}", tensor_id))?;
        if tensor.op != Op::FlashAttnExt {
            return Err(format!(
                "tensor '{}' is not a flash_attn_ext op",
                tensor.name().unwrap_or("<unnamed>")
            ));
        }
        tensor.set_op_param_i32(3, prec as i32);
        Ok(())
    }

    pub fn flash_attn_ext_add_sinks(
        &mut self,
        tensor_id: TensorId,
        sinks: Option<TensorId>,
    ) -> Result<(), String> {
        let tensor = self
            .tensor_mut(tensor_id)
            .ok_or_else(|| format!("invalid tensor id {}", tensor_id))?;
        if tensor.op != Op::FlashAttnExt {
            return Err(format!(
                "tensor '{}' is not a flash_attn_ext op",
                tensor.name().unwrap_or("<unnamed>")
            ));
        }
        tensor.src[4] = sinks;
        Ok(())
    }

    pub fn rms_norm(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        self.rms_norm_eps(src, 0.0, usage)
    }

    pub fn rms_norm_eps(
        &mut self,
        src: TensorId,
        eps: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let id = self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::RmsNorm,
            &[src],
        )?;
        self.tensor_mut(id).unwrap().set_op_param_f32(0, eps);
        Ok(id)
    }

    pub fn l2_norm(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        self.l2_norm_eps(src, 0.0, usage)
    }

    pub fn l2_norm_eps(
        &mut self,
        src: TensorId,
        eps: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let id = self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::L2Norm,
            &[src],
        )?;
        self.tensor_mut(id).unwrap().set_op_param_f32(0, eps);
        Ok(id)
    }

    pub fn rope(
        &mut self,
        src: TensorId,
        positions: TensorId,
        n_dims: i32,
        mode: i32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.rope_ext(
            src,
            positions,
            None,
            n_dims,
            mode,
            0,
            10_000.0,
            1.0,
            0.0,
            1.0,
            0.0,
            0.0,
            usage,
        )
    }

    pub fn rope_inplace(
        &mut self,
        src: TensorId,
        positions: TensorId,
        n_dims: i32,
        mode: i32,
    ) -> Result<TensorId, String> {
        self.rope_ext_inplace(
            src,
            positions,
            None,
            n_dims,
            mode,
            0,
            10_000.0,
            1.0,
            0.0,
            1.0,
            0.0,
            0.0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rope_ext(
        &mut self,
        src: TensorId,
        positions: TensorId,
        freq_factors: Option<TensorId>,
        n_dims: i32,
        mode: i32,
        n_ctx_orig: i32,
        freq_base: f32,
        freq_scale: f32,
        ext_factor: f32,
        attn_factor: f32,
        beta_fast: f32,
        beta_slow: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.rope_impl(
            src,
            positions,
            freq_factors,
            n_dims,
            None,
            mode,
            n_ctx_orig,
            freq_base,
            freq_scale,
            ext_factor,
            attn_factor,
            beta_fast,
            beta_slow,
            false,
            Some(usage),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rope_ext_inplace(
        &mut self,
        src: TensorId,
        positions: TensorId,
        freq_factors: Option<TensorId>,
        n_dims: i32,
        mode: i32,
        n_ctx_orig: i32,
        freq_base: f32,
        freq_scale: f32,
        ext_factor: f32,
        attn_factor: f32,
        beta_fast: f32,
        beta_slow: f32,
    ) -> Result<TensorId, String> {
        self.rope_impl(
            src,
            positions,
            freq_factors,
            n_dims,
            None,
            mode,
            n_ctx_orig,
            freq_base,
            freq_scale,
            ext_factor,
            attn_factor,
            beta_fast,
            beta_slow,
            true,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rope_multi(
        &mut self,
        src: TensorId,
        positions: TensorId,
        freq_factors: Option<TensorId>,
        n_dims: i32,
        sections: [i32; GGML_MROPE_SECTIONS],
        mode: i32,
        n_ctx_orig: i32,
        freq_base: f32,
        freq_scale: f32,
        ext_factor: f32,
        attn_factor: f32,
        beta_fast: f32,
        beta_slow: f32,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        self.rope_impl(
            src,
            positions,
            freq_factors,
            n_dims,
            Some(sections),
            mode,
            n_ctx_orig,
            freq_base,
            freq_scale,
            ext_factor,
            attn_factor,
            beta_fast,
            beta_slow,
            false,
            Some(usage),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn rope_multi_inplace(
        &mut self,
        src: TensorId,
        positions: TensorId,
        freq_factors: Option<TensorId>,
        n_dims: i32,
        sections: [i32; GGML_MROPE_SECTIONS],
        mode: i32,
        n_ctx_orig: i32,
        freq_base: f32,
        freq_scale: f32,
        ext_factor: f32,
        attn_factor: f32,
        beta_fast: f32,
        beta_slow: f32,
    ) -> Result<TensorId, String> {
        self.rope_impl(
            src,
            positions,
            freq_factors,
            n_dims,
            Some(sections),
            mode,
            n_ctx_orig,
            freq_base,
            freq_scale,
            ext_factor,
            attn_factor,
            beta_fast,
            beta_slow,
            true,
            None,
        )
    }

    pub fn pad(
        &mut self,
        src: TensorId,
        left: i64,
        right: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let mut ne = tensor.ne;
        ne[0] += left + right;
        let layout = TensorLayout::for_ggml(tensor.desc.ty, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, usage),
            Op::Pad,
            &[src],
        )
    }

    pub fn argsort(
        &mut self,
        src: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let layout = TensorLayout::for_ggml(TensorType::I32, &tensor.ne)?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::I32, layout, usage),
            Op::Argsort,
            &[src],
        )
    }

    pub fn top_k(
        &mut self,
        src: TensorId,
        k: i64,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let ne = [k, tensor.ne[1], tensor.ne[2], tensor.ne[3]];
        let layout = TensorLayout::for_ggml(TensorType::I32, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::I32, layout, usage),
            Op::TopK,
            &[src],
        )
    }

    pub fn ssm_conv(
        &mut self,
        sx: TensorId,
        c: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let sx_tensor = self
            .tensor(sx)
            .ok_or_else(|| format!("invalid tensor id {}", sx))?;
        let c_tensor = self
            .tensor(c)
            .ok_or_else(|| format!("invalid tensor id {}", c))?;
        if sx_tensor.desc.layout.rank() != 3 {
            return Err("ssm_conv requires a 3D source tensor".to_string());
        }
        if c_tensor.desc.layout.rank() != 2 {
            return Err("ssm_conv requires a 2D kernel tensor".to_string());
        }
        let d_conv = c_tensor.ne[0];
        let d_inner = c_tensor.ne[1];
        let n_t = sx_tensor
            .ne[0]
            .checked_sub(d_conv)
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| "ssm_conv token extent underflow".to_string())?;
        if sx_tensor.ne[1] != d_inner {
            return Err(format!(
                "ssm_conv source dim1 {} does not match kernel dim1 {}",
                sx_tensor.ne[1], d_inner
            ));
        }
        let layout = TensorLayout::for_ggml(TensorType::F32, &[d_inner, n_t, sx_tensor.ne[2]])?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::F32, layout, usage),
            Op::SsmConv,
            &[sx, c],
        )
    }

    pub fn gated_delta_net(
        &mut self,
        q: TensorId,
        k: TensorId,
        v: TensorId,
        g: TensorId,
        beta: TensorId,
        state: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let v_tensor = self
            .tensor(v)
            .ok_or_else(|| format!("invalid tensor id {}", v))?;
        let state_tensor = self
            .tensor(state)
            .ok_or_else(|| format!("invalid tensor id {}", state))?;
        let sv = v_tensor.ne[0];
        let h = v_tensor.ne[1];
        let n_tokens = v_tensor.ne[2];
        let n_seqs = v_tensor.ne[3];
        let expected_state = sv
            .checked_mul(sv)
            .and_then(|x| x.checked_mul(h))
            .and_then(|x| x.checked_mul(n_seqs))
            .ok_or_else(|| "gated_delta_net state element overflow".to_string())?;
        if state_tensor.nelements() != expected_state {
            return Err(format!(
                "gated_delta_net state elements {} do not match expected {}",
                state_tensor.nelements(),
                expected_state
            ));
        }
        let layout = TensorLayout::for_ggml(
            TensorType::F32,
            &[sv * h, n_tokens * n_seqs + sv * n_seqs, 1, 1],
        )?;
        self.new_op_tensor(
            TensorDesc::new(TensorType::F32, layout, usage),
            Op::GatedDeltaNet,
            &[q, k, v, g, beta, state],
        )
    }

    fn push_tensor(&mut self, mut tensor: Tensor, alloc_data: bool) -> Result<TensorId, String> {
        let nbytes = tensor.nbytes();
        self.max_tensor_size = self.max_tensor_size.max(nbytes);

        if alloc_data && !self.no_alloc && nbytes > 0 {
            let offset = ggml_pad(self.next_data_offset, GGML_MEM_ALIGN);
            let end = offset
                .checked_add(nbytes)
                .ok_or_else(|| "tensor allocation overflow".to_string())?;
            if end > self.mem_size {
                return Err(format!(
                    "context out of memory allocating {} bytes for tensor",
                    nbytes
                ));
            }
            tensor.data_offset = Some(offset);
            self.next_data_offset = end;
        }

        let id = tensor.id;
        if let Some(name) = tensor.name() {
            self.name_to_tensor.insert(name.to_string(), id);
        }

        self.tensors.push(tensor);
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn rope_impl(
        &mut self,
        src: TensorId,
        positions: TensorId,
        freq_factors: Option<TensorId>,
        n_dims: i32,
        sections: Option<[i32; GGML_MROPE_SECTIONS]>,
        mode: i32,
        n_ctx_orig: i32,
        freq_base: f32,
        freq_scale: f32,
        ext_factor: f32,
        attn_factor: f32,
        beta_fast: f32,
        beta_slow: f32,
        inplace: bool,
        usage: Option<BufferUsage>,
    ) -> Result<TensorId, String> {
        let src_tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let pos_tensor = self
            .tensor(positions)
            .ok_or_else(|| format!("invalid tensor id {}", positions))?;

        if pos_tensor.desc.ty != TensorType::I32 {
            return Err(format!(
                "rope positions tensor must be I32, got {}",
                pos_tensor.desc.ty.name()
            ));
        }

        if n_dims < 0 {
            return Err(format!("rope n_dims must be non-negative, got {}", n_dims));
        }

        if let Some(freq_factors) = freq_factors {
            let freq_tensor = self
                .tensor(freq_factors)
                .ok_or_else(|| format!("invalid tensor id {}", freq_factors))?;
            if freq_tensor.desc.ty != TensorType::F32 {
                return Err(format!(
                    "rope freq_factors tensor must be F32, got {}",
                    freq_tensor.desc.ty.name()
                ));
            }
            if freq_tensor.ne[0] < i64::from(n_dims / 2) {
                return Err(format!(
                    "rope freq_factors is too short: need at least {}, got {}",
                    n_dims / 2,
                    freq_tensor.ne[0]
                ));
            }
        }

        let desc = TensorDesc::new(
            src_tensor.desc.ty,
            src_tensor.desc.layout.clone(),
            usage.unwrap_or(src_tensor.desc.usage),
        );
        let mut srcs = Vec::with_capacity(3);
        srcs.push(src);
        srcs.push(positions);
        if let Some(freq_factors) = freq_factors {
            srcs.push(freq_factors);
        }

        let id = if inplace {
            self.new_view_op_tensor_with_srcs(desc, Op::Rope, src, 0, &srcs)?
        } else {
            self.new_op_tensor(desc, Op::Rope, &srcs)?
        };

        let tensor = self.tensor_mut(id).unwrap();
        tensor.set_op_param_i32(0, 0);
        tensor.set_op_param_i32(1, n_dims);
        tensor.set_op_param_i32(2, mode);
        tensor.set_op_param_i32(3, 0);
        tensor.set_op_param_i32(4, n_ctx_orig);
        tensor.set_op_param_f32(5, freq_base);
        tensor.set_op_param_f32(6, freq_scale);
        tensor.set_op_param_f32(7, ext_factor);
        tensor.set_op_param_f32(8, attn_factor);
        tensor.set_op_param_f32(9, beta_fast);
        tensor.set_op_param_f32(10, beta_slow);
        for (i, section) in sections.unwrap_or([0; GGML_MROPE_SECTIONS]).into_iter().enumerate() {
            tensor.set_op_param_i32(11 + i, section);
        }

        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE};
    use crate::tensor::ggml_row_size_for_type;

    #[test]
    fn allocates_basic_tensor_metadata() {
        let mut ctx = Context::new(InitParams {
            mem_size: 4096,
            mem_buffer: None,
            no_alloc: false,
        });

        let tensor = ctx
            .new_tensor_2d(TensorType::F32, 16, 8, BufferUsage::Activations)
            .unwrap();

        let t = ctx.tensor(tensor).unwrap();
        assert_eq!(t.ne, [16, 8, 1, 1]);
        assert_eq!(t.nb[0], 4);
        assert!(t.data_offset.is_some());
    }

    #[test]
    fn mul_mat_uses_upstream_result_shape() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 20,
            mem_buffer: None,
            no_alloc: true,
        });

        let a = ctx
            .new_tensor_2d(TensorType::F16, 128, 64, BufferUsage::Weights)
            .unwrap();
        let b = ctx
            .new_tensor_2d(TensorType::F16, 128, 32, BufferUsage::Activations)
            .unwrap();

        let out = ctx.mul_mat(a, b, BufferUsage::Activations).unwrap();
        let t = ctx.tensor(out).unwrap();
        assert_eq!(t.ne, [64, 32, 1, 1]);
        assert_eq!(t.desc.ty, TensorType::F32);
        assert_eq!(ggml_row_size_for_type(t.desc.ty, t.ne[0]).unwrap(), 256);
    }

    #[test]
    fn writes_named_tensor_bytes() {
        let mut ctx = Context::new(InitParams {
            mem_size: 4096,
            mem_buffer: None,
            no_alloc: false,
        });

        let tensor = ctx
            .new_named_tensor("weights.test", TensorType::F32, 2, &[4, 2], BufferUsage::Weights)
            .unwrap();
        ctx.write_tensor_data(tensor, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32])
            .unwrap();

        assert_eq!(ctx.get_tensor("weights.test"), Some(tensor));
        assert_eq!(ctx.tensor_data(tensor).unwrap()[0], 1);
        assert_eq!(ctx.tensor_data(tensor).unwrap()[31], 32);
    }

    #[test]
    fn op_tensors_allocate_data_when_context_allocates() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: false,
        });

        let a = ctx
            .new_tensor_2d(TensorType::F32, 8, 4, BufferUsage::Weights)
            .unwrap();
        let b = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.mul_mat(a, b, BufferUsage::Activations).unwrap();

        assert!(ctx.tensor(out).unwrap().data_offset.is_some());
        assert_eq!(ctx.tensor_data(out).unwrap().len(), ctx.tensor(out).unwrap().nbytes());
    }

    #[test]
    fn reshape_creates_view_tensor() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 8, 4, BufferUsage::Activations)
            .unwrap();
        let reshaped = ctx.reshape(src, &[4, 8]).unwrap();

        let src_tensor = ctx.tensor(src).unwrap();
        let reshaped_tensor = ctx.tensor(reshaped).unwrap();
        assert!(reshaped_tensor.is_view());
        assert_eq!(reshaped_tensor.view_src, Some(src));
        assert_eq!(reshaped_tensor.data_offset, src_tensor.data_offset);
    }

    #[test]
    fn cont_materializes_contiguous_layout() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 16, 2, BufferUsage::Activations)
            .unwrap();
        let view = ctx.view_3d(src, 4, 2, 2, 32, 64, 16).unwrap();
        let cont = ctx.cont_2d(view, 8, 2).unwrap();

        let view_tensor = ctx.tensor(view).unwrap();
        let cont_tensor = ctx.tensor(cont).unwrap();
        assert!(!view_tensor.is_contiguous());
        assert!(cont_tensor.is_contiguous());
        assert_eq!(cont_tensor.ne, [8, 2, 1, 1]);
        assert_eq!(cont_tensor.nb[0], 4);
        assert_eq!(cont_tensor.nb[1], 32);
    }

    #[test]
    fn get_rows_matches_upstream_output_type() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: true,
        });

        let src_q = ctx
            .new_tensor_2d(TensorType::Q5K, 256, 4, BufferUsage::Weights)
            .unwrap();
        let rows = ctx
            .new_tensor_1d(TensorType::I32, 2, BufferUsage::Activations)
            .unwrap();
        let out_q = ctx.get_rows(src_q, rows, BufferUsage::Activations).unwrap();
        assert_eq!(ctx.tensor(out_q).unwrap().desc.ty, TensorType::F32);

        let src_i = ctx
            .new_tensor_2d(TensorType::I32, 8, 4, BufferUsage::Weights)
            .unwrap();
        let out_i = ctx.get_rows(src_i, rows, BufferUsage::Activations).unwrap();
        assert_eq!(ctx.tensor(out_i).unwrap().desc.ty, TensorType::I32);
    }

    #[test]
    fn set_rows_creates_inplace_view_op() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: true,
        });

        let dst = ctx
            .new_tensor_2d(TensorType::F16, 8, 4, BufferUsage::State)
            .unwrap();
        let src = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let rows = ctx
            .new_tensor_1d(TensorType::I32, 2, BufferUsage::Activations)
            .unwrap();
        let out = ctx.set_rows(dst, src, rows, BufferUsage::State).unwrap();

        let tensor = ctx.tensor(out).unwrap();
        assert_eq!(tensor.op, Op::SetRows);
        assert!(tensor.is_view());
        assert_eq!(tensor.view_src, Some(dst));
        assert_eq!(tensor.src[0], Some(src));
        assert_eq!(tensor.src[1], Some(rows));
        assert_eq!(tensor.src[2], Some(dst));
        assert_eq!(tensor.desc.ty, TensorType::F16);
        assert_eq!(tensor.desc.usage, BufferUsage::State);
    }

    #[test]
    fn rope_multi_stores_upstream_params() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: true,
        });

        let src = ctx
            .new_tensor_3d(TensorType::F32, 128, 8, 2, BufferUsage::Activations)
            .unwrap();
        let pos = ctx
            .new_tensor_2d(TensorType::I32, 8, 4, BufferUsage::Activations)
            .unwrap();
        let freq = ctx
            .new_tensor_1d(TensorType::F32, 32, BufferUsage::Weights)
            .unwrap();

        let rope = ctx
            .rope_multi(
                src,
                pos,
                Some(freq),
                64,
                [11, 11, 10, 0],
                GGML_ROPE_TYPE_MROPE | GGML_ROPE_TYPE_IMROPE,
                262_144,
                1_000_000.0,
                0.5,
                0.25,
                1.5,
                32.0,
                1.0,
                BufferUsage::Activations,
            )
            .unwrap();

        let tensor = ctx.tensor(rope).unwrap();
        assert_eq!(tensor.op, Op::Rope);
        assert_eq!(tensor.src[0], Some(src));
        assert_eq!(tensor.src[1], Some(pos));
        assert_eq!(tensor.src[2], Some(freq));
        assert_eq!(tensor.op_param_i32(1), 64);
        assert_eq!(tensor.op_param_i32(2), GGML_ROPE_TYPE_MROPE | GGML_ROPE_TYPE_IMROPE);
        assert_eq!(tensor.op_param_i32(4), 262_144);
        assert_eq!(tensor.op_param_i32(11), 11);
        assert_eq!(tensor.op_param_i32(12), 11);
        assert_eq!(tensor.op_param_i32(13), 10);
        assert_eq!(tensor.op_param_i32(14), 0);
        assert_eq!(tensor.op_param_f32(5), 1_000_000.0);
        assert_eq!(tensor.op_param_f32(6), 0.5);
        assert_eq!(tensor.op_param_f32(7), 0.25);
        assert_eq!(tensor.op_param_f32(8), 1.5);
        assert_eq!(tensor.op_param_f32(9), 32.0);
        assert_eq!(tensor.op_param_f32(10), 1.0);
    }

    #[test]
    fn rope_inplace_creates_view_op() {
        let mut ctx = Context::new(InitParams {
            mem_size: 1 << 12,
            mem_buffer: None,
            no_alloc: false,
        });

        let src = ctx
            .new_tensor_2d(TensorType::F32, 8, 2, BufferUsage::Activations)
            .unwrap();
        let pos = ctx
            .new_tensor_1d(TensorType::I32, 2, BufferUsage::Activations)
            .unwrap();
        let rope = ctx.rope_inplace(src, pos, 8, 0).unwrap();

        let tensor = ctx.tensor(rope).unwrap();
        assert_eq!(tensor.op, Op::Rope);
        assert_eq!(tensor.view_src, Some(src));
        assert_eq!(tensor.src[0], Some(src));
        assert_eq!(tensor.src[1], Some(pos));
        assert_eq!(tensor.data_offset, ctx.tensor(src).unwrap().data_offset);
    }
}
