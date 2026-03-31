use std::collections::HashMap;

use crate::core::{ggml_pad, InitParams, GGML_MEM_ALIGN};
use crate::op::{GluOp, Op, UnaryOp};
use crate::tensor::{
    BufferUsage, Tensor, TensorDesc, TensorId, TensorLayout, TensorType,
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
        self.push_tensor(tensor, false)
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

    pub fn reshape(
        &mut self,
        src: TensorId,
        extents: &[i64],
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let layout = TensorLayout::for_ggml(tensor.desc.ty, extents)?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, tensor.desc.usage),
            Op::Reshape,
            &[src],
        )
    }

    pub fn cont(&mut self, src: TensorId) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), tensor.desc.usage),
            Op::Cont,
            &[src],
        )
    }

    pub fn permute(&mut self, src: TensorId, axes: [usize; 4]) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        let mut extents = [1_i64; 4];
        let mut strides = [0_usize; 4];
        for (dst_idx, src_idx) in axes.into_iter().enumerate() {
            extents[dst_idx] = tensor.ne[src_idx];
            strides[dst_idx] = tensor.nb[src_idx];
        }
        let layout = TensorLayout::from_parts(4, &extents, &strides)?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, tensor.desc.usage),
            Op::Permute,
            &[src],
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
        let layout = TensorLayout::for_ggml(tensor.desc.ty, &ne)?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, layout, usage),
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
        let tensor = self
            .tensor(dst)
            .ok_or_else(|| format!("invalid tensor id {}", dst))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::SetRows,
            &[dst, src, rows],
        )
    }

    pub fn soft_max(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::SoftMax,
            &[src],
        )
    }

    pub fn rms_norm(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::RmsNorm,
            &[src],
        )
    }

    pub fn l2_norm(&mut self, src: TensorId, usage: BufferUsage) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::L2Norm,
            &[src],
        )
    }

    pub fn rope(
        &mut self,
        src: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let tensor = self
            .tensor(src)
            .ok_or_else(|| format!("invalid tensor id {}", src))?;
        self.new_op_tensor(
            TensorDesc::new(tensor.desc.ty, tensor.desc.layout.clone(), usage),
            Op::Rope,
            &[src],
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
        a: TensorId,
        b: TensorId,
        c: TensorId,
        usage: BufferUsage,
    ) -> Result<TensorId, String> {
        let src = self
            .tensor(a)
            .ok_or_else(|| format!("invalid tensor id {}", a))?;
        self.new_op_tensor(
            TensorDesc::new(src.desc.ty, src.desc.layout.clone(), usage),
            Op::SsmConv,
            &[a, b, c],
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
        let src = self
            .tensor(v)
            .ok_or_else(|| format!("invalid tensor id {}", v))?;
        self.new_op_tensor(
            TensorDesc::new(src.desc.ty, src.desc.layout.clone(), usage),
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
