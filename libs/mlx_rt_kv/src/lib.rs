pub mod layer0_cached_case;

use makepad_mlx_rt_core::MlxTextConfig;
use std::fmt;

pub type Result<T> = std::result::Result<T, GemmaKvError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemmaAttentionKind {
    Full,
    Sliding,
}

impl GemmaAttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full_attention",
            Self::Sliding => "sliding_attention",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KvTensorShape {
    pub batch_size: usize,
    pub kv_head_count: usize,
    pub seq_len: usize,
    pub head_dim: usize,
}

impl KvTensorShape {
    fn validate(self) -> Result<()> {
        if self.batch_size == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache batch_size must be greater than zero",
            ));
        }
        if self.kv_head_count == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache kv_head_count must be greater than zero",
            ));
        }
        if self.head_dim == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache head_dim must be greater than zero",
            ));
        }
        Ok(())
    }

    pub fn row_count(self) -> Result<usize> {
        checked_product(
            &[self.batch_size, self.kv_head_count],
            "KV cache row count overflow",
        )
    }

    pub fn contiguous_row_stride_elems(self) -> Result<usize> {
        checked_product(
            &[self.seq_len, self.head_dim],
            "KV tensor row stride overflow",
        )
    }

    pub fn element_count(self) -> Result<usize> {
        checked_product(
            &[
                self.batch_size,
                self.kv_head_count,
                self.seq_len,
                self.head_dim,
            ],
            "KV tensor element count overflow",
        )
    }
}

impl fmt::Display for KvTensorShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[batch={}, heads={}, seq={}, dim={}]",
            self.batch_size, self.kv_head_count, self.seq_len, self.head_dim
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KvTensor<T> {
    shape: KvTensorShape,
    data: Vec<T>,
}

impl<T> KvTensor<T> {
    pub fn from_vec(shape: KvTensorShape, data: Vec<T>) -> Result<Self> {
        shape.validate()?;
        let expected = shape.element_count()?;
        if data.len() != expected {
            return Err(GemmaKvError::DataLengthMismatch {
                context: "KV tensor",
                expected,
                actual: data.len(),
            });
        }
        Ok(Self { shape, data })
    }

    pub fn shape(&self) -> KvTensorShape {
        self.shape
    }

    pub fn data(&self) -> &[T] {
        &self.data
    }

    pub fn view(&self) -> KvTensorView<'_, T> {
        KvTensorView::from_parts(
            self.shape,
            self.shape
                .contiguous_row_stride_elems()
                .expect("contiguous row stride must fit"),
            &self.data,
        )
        .expect("owned tensor must always produce a valid contiguous view")
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KvTensorView<'a, T> {
    shape: KvTensorShape,
    row_stride_elems: usize,
    data: &'a [T],
}

impl<'a, T> KvTensorView<'a, T> {
    pub fn from_parts(
        shape: KvTensorShape,
        row_stride_elems: usize,
        data: &'a [T],
    ) -> Result<Self> {
        shape.validate()?;
        let minimum_row_stride = shape.contiguous_row_stride_elems()?;
        if row_stride_elems < minimum_row_stride {
            return Err(GemmaKvError::InvalidViewStride {
                row_stride_elems,
                minimum_row_stride_elems: minimum_row_stride,
            });
        }
        let expected = checked_product(
            &[shape.row_count()?, row_stride_elems],
            "KV view backing length overflow",
        )?;
        if data.len() != expected {
            return Err(GemmaKvError::DataLengthMismatch {
                context: "KV tensor view",
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            shape,
            row_stride_elems,
            data,
        })
    }

    pub fn shape(&self) -> KvTensorShape {
        self.shape
    }

    pub fn row_stride_elems(&self) -> usize {
        self.row_stride_elems
    }

    pub fn is_contiguous(&self) -> bool {
        self.row_stride_elems
            == self
                .shape
                .contiguous_row_stride_elems()
                .expect("validated shape row stride must fit")
    }

    pub fn row(&self, batch: usize, head: usize, token: usize) -> Result<&'a [T]> {
        if batch >= self.shape.batch_size {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "batch",
                index: batch,
                len: self.shape.batch_size,
            });
        }
        if head >= self.shape.kv_head_count {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "head",
                index: head,
                len: self.shape.kv_head_count,
            });
        }
        if token >= self.shape.seq_len {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "token",
                index: token,
                len: self.shape.seq_len,
            });
        }
        let row_index = batch
            .checked_mul(self.shape.kv_head_count)
            .and_then(|value| value.checked_add(head))
            .ok_or(GemmaKvError::Overflow("KV row index overflow"))?;
        let row_base = row_index
            .checked_mul(self.row_stride_elems)
            .ok_or(GemmaKvError::Overflow("KV row base overflow"))?;
        let token_base = token
            .checked_mul(self.shape.head_dim)
            .ok_or(GemmaKvError::Overflow("KV token base overflow"))?;
        let start = row_base
            .checked_add(token_base)
            .ok_or(GemmaKvError::Overflow("KV slice start overflow"))?;
        let end = start
            .checked_add(self.shape.head_dim)
            .ok_or(GemmaKvError::Overflow("KV slice end overflow"))?;
        Ok(&self.data[start..end])
    }

    pub fn get(&self, batch: usize, head: usize, token: usize, dim: usize) -> Result<&'a T> {
        if dim >= self.shape.head_dim {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "dim",
                index: dim,
                len: self.shape.head_dim,
            });
        }
        let row = self.row(batch, head, token)?;
        Ok(&row[dim])
    }

    pub fn to_tensor(&self) -> Result<KvTensor<T>>
    where
        T: Clone,
    {
        let mut data = Vec::with_capacity(self.shape.element_count()?);
        for batch in 0..self.shape.batch_size {
            for head in 0..self.shape.kv_head_count {
                for token in 0..self.shape.seq_len {
                    data.extend_from_slice(self.row(batch, head, token)?);
                }
            }
        }
        KvTensor::from_vec(self.shape, data)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GemmaKvCacheSpec {
    pub attention: GemmaAttentionKind,
    pub batch_size: usize,
    pub kv_head_count: usize,
    pub head_dim: usize,
    pub max_tokens: usize,
}

impl GemmaKvCacheSpec {
    pub fn new(
        attention: GemmaAttentionKind,
        batch_size: usize,
        kv_head_count: usize,
        head_dim: usize,
        max_tokens: usize,
    ) -> Result<Self> {
        if batch_size == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache batch_size must be greater than zero",
            ));
        }
        if kv_head_count == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache kv_head_count must be greater than zero",
            ));
        }
        if head_dim == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache head_dim must be greater than zero",
            ));
        }
        if max_tokens == 0 {
            return Err(GemmaKvError::InvalidConfig(
                "KV cache max_tokens must be greater than zero",
            ));
        }
        Ok(Self {
            attention,
            batch_size,
            kv_head_count,
            head_dim,
            max_tokens,
        })
    }

    pub fn tensor_shape(&self, seq_len: usize) -> Result<KvTensorShape> {
        let shape = KvTensorShape {
            batch_size: self.batch_size,
            kv_head_count: self.kv_head_count,
            seq_len,
            head_dim: self.head_dim,
        };
        shape.validate()?;
        Ok(shape)
    }

    fn row_stride_elems(&self) -> Result<usize> {
        checked_product(
            &[self.max_tokens, self.head_dim],
            "KV cache row stride overflow",
        )
    }

    fn storage_len(&self) -> Result<usize> {
        checked_product(
            &[
                self.batch_size,
                self.kv_head_count,
                self.max_tokens,
                self.head_dim,
            ],
            "KV cache storage length overflow",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GemmaKvCacheLayout {
    pub cache_specs: Vec<GemmaKvCacheSpec>,
    pub layer_idx_to_cache_idx: Vec<usize>,
    pub first_kv_shared_layer_idx: usize,
    pub first_full_cache_idx: Option<usize>,
    pub first_sliding_cache_idx: Option<usize>,
}

impl GemmaKvCacheLayout {
    pub fn from_text_config(config: &MlxTextConfig, batch_size: usize) -> Result<Self> {
        if config.layer_types.is_empty() {
            return Err(GemmaKvError::InvalidConfig(
                "Gemma text config must define at least one layer type",
            ));
        }

        let layer_count = checked_usize(config.num_hidden_layers, "num_hidden_layers")?;
        if config.layer_types.len() != layer_count {
            return Err(GemmaKvError::InvalidConfig(
                "layer_types length must match num_hidden_layers",
            ));
        }

        let shared_layer_count =
            checked_usize(config.num_kv_shared_layers, "num_kv_shared_layers")?;
        if shared_layer_count > layer_count {
            return Err(GemmaKvError::InvalidConfig(
                "num_kv_shared_layers cannot exceed num_hidden_layers",
            ));
        }
        if shared_layer_count == layer_count {
            return Err(GemmaKvError::InvalidConfig(
                "Gemma KV sharing requires at least one concrete cache-owning layer",
            ));
        }

        let kv_head_count = checked_usize(config.num_key_value_heads, "num_key_value_heads")?;
        let head_dim = checked_usize(config.head_dim, "head_dim")?;
        let max_position_embeddings =
            checked_usize(config.max_position_embeddings, "max_position_embeddings")?;
        let sliding_window = checked_usize(config.sliding_window, "sliding_window")?;
        let first_kv_shared_layer_idx = layer_count - shared_layer_count;

        let mut cache_specs = Vec::with_capacity(first_kv_shared_layer_idx);
        let mut concrete_attention = Vec::with_capacity(first_kv_shared_layer_idx);
        for (layer_idx, layer_type) in config
            .layer_types
            .iter()
            .take(first_kv_shared_layer_idx)
            .enumerate()
        {
            let attention = parse_attention_kind(layer_idx, layer_type)?;
            let max_tokens = match attention {
                GemmaAttentionKind::Full => max_position_embeddings,
                GemmaAttentionKind::Sliding => sliding_window,
            };
            cache_specs.push(GemmaKvCacheSpec::new(
                attention,
                batch_size,
                kv_head_count,
                head_dim,
                max_tokens,
            )?);
            concrete_attention.push(attention);
        }

        let mut layer_idx_to_cache_idx = (0..first_kv_shared_layer_idx).collect::<Vec<_>>();
        for (layer_idx, layer_type) in config
            .layer_types
            .iter()
            .enumerate()
            .skip(first_kv_shared_layer_idx)
        {
            let attention = parse_attention_kind(layer_idx, layer_type)?;
            let cache_idx = concrete_attention
                .iter()
                .rposition(|candidate| *candidate == attention)
                .ok_or(GemmaKvError::MissingConcreteCache {
                    layer_idx,
                    attention,
                })?;
            layer_idx_to_cache_idx.push(cache_idx);
        }

        let first_full_cache_idx = concrete_attention
            .iter()
            .position(|candidate| *candidate == GemmaAttentionKind::Full);
        let first_sliding_cache_idx = concrete_attention
            .iter()
            .position(|candidate| *candidate == GemmaAttentionKind::Sliding);

        Ok(Self {
            cache_specs,
            layer_idx_to_cache_idx,
            first_kv_shared_layer_idx,
            first_full_cache_idx,
            first_sliding_cache_idx,
        })
    }

    pub fn layer_count(&self) -> usize {
        self.layer_idx_to_cache_idx.len()
    }

    pub fn cache_count(&self) -> usize {
        self.cache_specs.len()
    }

    pub fn is_kv_shared_layer(&self, layer_idx: usize) -> Result<bool> {
        if layer_idx >= self.layer_count() {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "layer",
                index: layer_idx,
                len: self.layer_count(),
            });
        }
        Ok(layer_idx >= self.first_kv_shared_layer_idx)
    }

    pub fn cache_idx_for_layer(&self, layer_idx: usize) -> Result<usize> {
        if layer_idx >= self.layer_count() {
            return Err(GemmaKvError::IndexOutOfRange {
                axis: "layer",
                index: layer_idx,
                len: self.layer_count(),
            });
        }
        Ok(self.layer_idx_to_cache_idx[layer_idx])
    }

    pub fn cache_spec_for_layer(&self, layer_idx: usize) -> Result<&GemmaKvCacheSpec> {
        let cache_idx = self.cache_idx_for_layer(layer_idx)?;
        Ok(&self.cache_specs[cache_idx])
    }
}

#[derive(Clone, Debug)]
pub struct GemmaKvStateView<'a, T> {
    pub keys: KvTensorView<'a, T>,
    pub values: KvTensorView<'a, T>,
    start_position: usize,
    next_position: usize,
}

impl<'a, T> GemmaKvStateView<'a, T> {
    pub fn stored_tokens(&self) -> usize {
        self.keys.shape().seq_len
    }

    pub fn start_position(&self) -> usize {
        self.start_position
    }

    pub fn next_position(&self) -> usize {
        self.next_position
    }

    pub fn offset(&self) -> usize {
        self.next_position
    }
}

#[derive(Clone, Debug)]
pub struct GemmaKvCache<T> {
    spec: GemmaKvCacheSpec,
    stored_tokens: usize,
    next_position: usize,
    keys: Vec<T>,
    values: Vec<T>,
}

impl<T: Copy + Default> GemmaKvCache<T> {
    pub fn new(spec: GemmaKvCacheSpec) -> Result<Self> {
        let storage_len = spec.storage_len()?;
        Ok(Self {
            spec,
            stored_tokens: 0,
            next_position: 0,
            keys: vec![T::default(); storage_len],
            values: vec![T::default(); storage_len],
        })
    }
}

impl<T: Copy> GemmaKvCache<T> {
    pub fn spec(&self) -> &GemmaKvCacheSpec {
        &self.spec
    }

    pub fn stored_tokens(&self) -> usize {
        self.stored_tokens
    }

    pub fn next_position(&self) -> usize {
        self.next_position
    }

    pub fn offset(&self) -> usize {
        self.next_position
    }

    pub fn start_position(&self) -> usize {
        self.next_position.saturating_sub(self.stored_tokens)
    }

    pub fn fetch(&self) -> Result<GemmaKvStateView<'_, T>> {
        let shape = self.spec.tensor_shape(self.stored_tokens)?;
        let row_stride = self.spec.row_stride_elems()?;
        Ok(GemmaKvStateView {
            keys: KvTensorView::from_parts(shape, row_stride, &self.keys)?,
            values: KvTensorView::from_parts(shape, row_stride, &self.values)?,
            start_position: self.start_position(),
            next_position: self.next_position,
        })
    }

    pub fn update_and_fetch(
        &mut self,
        keys: KvTensorView<'_, T>,
        values: KvTensorView<'_, T>,
    ) -> Result<GemmaKvStateView<'_, T>> {
        self.validate_update_shapes(keys, values)?;
        let seq_len = keys.shape().seq_len;
        if seq_len == 0 {
            return self.fetch();
        }

        match self.spec.attention {
            GemmaAttentionKind::Full => self.append_full(keys, values)?,
            GemmaAttentionKind::Sliding => self.append_sliding(keys, values)?,
        }

        self.next_position = self
            .next_position
            .checked_add(seq_len)
            .ok_or(GemmaKvError::Overflow("KV cache next_position overflow"))?;
        self.fetch()
    }

    fn validate_update_shapes(
        &self,
        keys: KvTensorView<'_, T>,
        values: KvTensorView<'_, T>,
    ) -> Result<()> {
        let expected = self.spec.tensor_shape(keys.shape().seq_len)?;
        if keys.shape() != expected {
            return Err(GemmaKvError::ShapeMismatch {
                context: "key update tensor",
                expected,
                actual: keys.shape(),
            });
        }
        if values.shape() != expected {
            return Err(GemmaKvError::ShapeMismatch {
                context: "value update tensor",
                expected,
                actual: values.shape(),
            });
        }
        Ok(())
    }

    fn append_full(
        &mut self,
        keys: KvTensorView<'_, T>,
        values: KvTensorView<'_, T>,
    ) -> Result<()> {
        let seq_len = keys.shape().seq_len;
        let new_len = self
            .stored_tokens
            .checked_add(seq_len)
            .ok_or(GemmaKvError::Overflow(
                "KV cache full-append length overflow",
            ))?;
        if new_len > self.spec.max_tokens {
            return Err(GemmaKvError::CacheOverflow {
                attention: self.spec.attention,
                attempted_tokens: new_len,
                capacity_tokens: self.spec.max_tokens,
            });
        }

        let row_stride = self.spec.row_stride_elems()?;
        copy_tokens_into_storage(
            keys,
            &mut self.keys,
            row_stride,
            self.stored_tokens,
            0,
            seq_len,
        )?;
        copy_tokens_into_storage(
            values,
            &mut self.values,
            row_stride,
            self.stored_tokens,
            0,
            seq_len,
        )?;
        self.stored_tokens = new_len;
        Ok(())
    }

    fn append_sliding(
        &mut self,
        keys: KvTensorView<'_, T>,
        values: KvTensorView<'_, T>,
    ) -> Result<()> {
        let seq_len = keys.shape().seq_len;
        let capacity = self.spec.max_tokens;
        let row_stride = self.spec.row_stride_elems()?;

        if seq_len >= capacity {
            let keep_start = seq_len - capacity;
            copy_tokens_into_storage(keys, &mut self.keys, row_stride, 0, keep_start, capacity)?;
            copy_tokens_into_storage(
                values,
                &mut self.values,
                row_stride,
                0,
                keep_start,
                capacity,
            )?;
            self.stored_tokens = capacity;
            return Ok(());
        }

        let new_len = self
            .stored_tokens
            .checked_add(seq_len)
            .ok_or(GemmaKvError::Overflow(
                "KV cache sliding-append length overflow",
            ))?;

        if new_len > capacity {
            let shift_tokens = new_len - capacity;
            self.shift_storage_left(shift_tokens)?;
            self.stored_tokens -= shift_tokens;
        }

        copy_tokens_into_storage(
            keys,
            &mut self.keys,
            row_stride,
            self.stored_tokens,
            0,
            seq_len,
        )?;
        copy_tokens_into_storage(
            values,
            &mut self.values,
            row_stride,
            self.stored_tokens,
            0,
            seq_len,
        )?;
        self.stored_tokens += seq_len;
        Ok(())
    }

    fn shift_storage_left(&mut self, shift_tokens: usize) -> Result<()> {
        if shift_tokens == 0 {
            return Ok(());
        }
        let row_stride = self.spec.row_stride_elems()?;
        let row_count = checked_product(
            &[self.spec.batch_size, self.spec.kv_head_count],
            "KV cache shift row count overflow",
        )?;
        let shift_elems = checked_product(
            &[shift_tokens, self.spec.head_dim],
            "KV cache shift element count overflow",
        )?;
        let remaining_tokens = self.stored_tokens.checked_sub(shift_tokens).unwrap_or(0);
        let remaining_elems = checked_product(
            &[remaining_tokens, self.spec.head_dim],
            "KV cache remaining element count overflow",
        )?;

        for row in 0..row_count {
            let row_base = row
                .checked_mul(row_stride)
                .ok_or(GemmaKvError::Overflow("KV cache row base overflow"))?;
            let src_start = row_base
                .checked_add(shift_elems)
                .ok_or(GemmaKvError::Overflow("KV cache shift src overflow"))?;
            let src_end = src_start
                .checked_add(remaining_elems)
                .ok_or(GemmaKvError::Overflow("KV cache shift src end overflow"))?;
            self.keys.copy_within(src_start..src_end, row_base);
            self.values.copy_within(src_start..src_end, row_base);
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct GemmaKvCacheSet<T> {
    layout: GemmaKvCacheLayout,
    caches: Vec<GemmaKvCache<T>>,
}

impl<T: Copy + Default> GemmaKvCacheSet<T> {
    pub fn new(layout: GemmaKvCacheLayout) -> Result<Self> {
        let mut caches = Vec::with_capacity(layout.cache_specs.len());
        for spec in &layout.cache_specs {
            caches.push(GemmaKvCache::new(spec.clone())?);
        }
        Ok(Self { layout, caches })
    }
}

impl<T> GemmaKvCacheSet<T> {
    pub fn layout(&self) -> &GemmaKvCacheLayout {
        &self.layout
    }

    pub fn cache(&self, cache_idx: usize) -> Result<&GemmaKvCache<T>> {
        self.caches
            .get(cache_idx)
            .ok_or(GemmaKvError::IndexOutOfRange {
                axis: "cache",
                index: cache_idx,
                len: self.caches.len(),
            })
    }

    pub fn cache_mut(&mut self, cache_idx: usize) -> Result<&mut GemmaKvCache<T>> {
        let len = self.caches.len();
        self.caches
            .get_mut(cache_idx)
            .ok_or(GemmaKvError::IndexOutOfRange {
                axis: "cache",
                index: cache_idx,
                len,
            })
    }

    pub fn cache_for_layer(&self, layer_idx: usize) -> Result<&GemmaKvCache<T>> {
        let cache_idx = self.layout.cache_idx_for_layer(layer_idx)?;
        self.cache(cache_idx)
    }

    pub fn cache_for_layer_mut(&mut self, layer_idx: usize) -> Result<&mut GemmaKvCache<T>> {
        let cache_idx = self.layout.cache_idx_for_layer(layer_idx)?;
        self.cache_mut(cache_idx)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GemmaKvError {
    InvalidConfig(&'static str),
    UnsupportedLayerType {
        layer_idx: usize,
        layer_type: String,
    },
    MissingConcreteCache {
        layer_idx: usize,
        attention: GemmaAttentionKind,
    },
    InvalidViewStride {
        row_stride_elems: usize,
        minimum_row_stride_elems: usize,
    },
    DataLengthMismatch {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    ShapeMismatch {
        context: &'static str,
        expected: KvTensorShape,
        actual: KvTensorShape,
    },
    CacheOverflow {
        attention: GemmaAttentionKind,
        attempted_tokens: usize,
        capacity_tokens: usize,
    },
    IndexOutOfRange {
        axis: &'static str,
        index: usize,
        len: usize,
    },
    Overflow(&'static str),
}

impl fmt::Display for GemmaKvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "{msg}"),
            Self::UnsupportedLayerType {
                layer_idx,
                layer_type,
            } => write!(
                f,
                "unsupported Gemma layer type at layer {layer_idx}: {layer_type}"
            ),
            Self::MissingConcreteCache {
                layer_idx,
                attention,
            } => write!(
                f,
                "shared Gemma layer {layer_idx} has no earlier {} cache to reuse",
                attention.as_str()
            ),
            Self::InvalidViewStride {
                row_stride_elems,
                minimum_row_stride_elems,
            } => write!(
                f,
                "invalid KV view row stride {row_stride_elems}; minimum is {minimum_row_stride_elems}"
            ),
            Self::DataLengthMismatch {
                context,
                expected,
                actual,
            } => write!(
                f,
                "{context} length mismatch: expected {expected} elements, got {actual}"
            ),
            Self::ShapeMismatch {
                context,
                expected,
                actual,
            } => write!(f, "{context} shape mismatch: expected {expected}, got {actual}"),
            Self::CacheOverflow {
                attention,
                attempted_tokens,
                capacity_tokens,
            } => write!(
                f,
                "{} KV cache overflow: attempted {} tokens with capacity {}",
                attention.as_str(),
                attempted_tokens,
                capacity_tokens
            ),
            Self::IndexOutOfRange { axis, index, len } => {
                write!(f, "{axis} index {index} out of range for length {len}")
            }
            Self::Overflow(context) => write!(f, "{context}"),
        }
    }
}

impl std::error::Error for GemmaKvError {}

fn parse_attention_kind(layer_idx: usize, layer_type: &str) -> Result<GemmaAttentionKind> {
    match layer_type {
        "full_attention" => Ok(GemmaAttentionKind::Full),
        "sliding_attention" => Ok(GemmaAttentionKind::Sliding),
        _ => Err(GemmaKvError::UnsupportedLayerType {
            layer_idx,
            layer_type: layer_type.to_owned(),
        }),
    }
}

fn checked_usize(value: u32, context: &'static str) -> Result<usize> {
    usize::try_from(value).map_err(|_| GemmaKvError::Overflow(context))
}

fn checked_product(parts: &[usize], context: &'static str) -> Result<usize> {
    let mut acc = 1usize;
    for part in parts {
        acc = acc
            .checked_mul(*part)
            .ok_or(GemmaKvError::Overflow(context))?;
    }
    Ok(acc)
}

fn copy_tokens_into_storage<T: Copy>(
    src: KvTensorView<'_, T>,
    dst: &mut [T],
    dst_row_stride_elems: usize,
    dst_start_token: usize,
    src_start_token: usize,
    token_count: usize,
) -> Result<()> {
    if token_count == 0 {
        return Ok(());
    }

    let shape = src.shape();
    let src_end_token = src_start_token
        .checked_add(token_count)
        .ok_or(GemmaKvError::Overflow("KV source token range overflow"))?;
    if src_end_token > shape.seq_len {
        return Err(GemmaKvError::InvalidConfig(
            "KV source token span exceeds the provided tensor",
        ));
    }

    let head_dim = shape.head_dim;
    let row_count = shape.row_count()?;
    let src_token_base = checked_product(
        &[src_start_token, head_dim],
        "KV source token base overflow",
    )?;
    let dst_token_base = checked_product(
        &[dst_start_token, head_dim],
        "KV destination token base overflow",
    )?;
    let copy_len = checked_product(&[token_count, head_dim], "KV copy span overflow")?;

    for row in 0..row_count {
        let src_row_base = row
            .checked_mul(src.row_stride_elems())
            .ok_or(GemmaKvError::Overflow("KV source row base overflow"))?;
        let dst_row_base = row
            .checked_mul(dst_row_stride_elems)
            .ok_or(GemmaKvError::Overflow("KV destination row base overflow"))?;
        let src_start = src_row_base
            .checked_add(src_token_base)
            .ok_or(GemmaKvError::Overflow("KV source start overflow"))?;
        let src_end = src_start
            .checked_add(copy_len)
            .ok_or(GemmaKvError::Overflow("KV source end overflow"))?;
        let dst_start = dst_row_base
            .checked_add(dst_token_base)
            .ok_or(GemmaKvError::Overflow("KV destination start overflow"))?;
        let dst_end = dst_start
            .checked_add(copy_len)
            .ok_or(GemmaKvError::Overflow("KV destination end overflow"))?;
        dst[dst_start..dst_end].copy_from_slice(&src.data[src_start..src_end]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_mlx_rt_core::{MlxRopeAttentionParameters, MlxTextRopeParameters};

    #[test]
    fn layout_reuses_last_concrete_cache_of_the_same_attention_kind() -> Result<()> {
        let config = sample_text_config(
            &[
                "full_attention",
                "sliding_attention",
                "full_attention",
                "sliding_attention",
                "full_attention",
                "sliding_attention",
            ],
            2,
        );
        let layout = GemmaKvCacheLayout::from_text_config(&config, 1)?;

        assert_eq!(layout.first_kv_shared_layer_idx, 4);
        assert_eq!(layout.layer_idx_to_cache_idx, vec![0, 1, 2, 3, 2, 3]);
        assert_eq!(layout.first_full_cache_idx, Some(0));
        assert_eq!(layout.first_sliding_cache_idx, Some(1));
        assert_eq!(layout.cache_specs.len(), 4);
        assert_eq!(layout.cache_specs[0].attention, GemmaAttentionKind::Full);
        assert_eq!(layout.cache_specs[1].attention, GemmaAttentionKind::Sliding);
        assert!(layout.is_kv_shared_layer(4)?);
        assert!(!layout.is_kv_shared_layer(3)?);
        Ok(())
    }

    #[test]
    fn full_cache_prefill_then_decode_preserves_head_layout() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Full, 1, 2, 3, 8)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let prefill_keys = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 2,
                seq_len: 2,
                head_dim: 3,
            },
            0,
        )?;
        let prefill_values = test_tensor(prefill_keys.shape(), 1000)?;
        cache.update_and_fetch(prefill_keys.view(), prefill_values.view())?;

        let decode_keys = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 2,
                seq_len: 1,
                head_dim: 3,
            },
            200,
        )?;
        let decode_values = test_tensor(decode_keys.shape(), 1200)?;
        let state = cache.update_and_fetch(decode_keys.view(), decode_values.view())?;

        assert_eq!(state.stored_tokens(), 3);
        assert_eq!(state.start_position(), 0);
        assert_eq!(state.offset(), 3);
        assert_eq!(state.keys.row(0, 0, 0)?, &[0, 1, 2]);
        assert_eq!(state.keys.row(0, 0, 1)?, &[10, 11, 12]);
        assert_eq!(state.keys.row(0, 0, 2)?, &[200, 201, 202]);
        assert_eq!(state.keys.row(0, 1, 0)?, &[100, 101, 102]);
        assert_eq!(state.keys.row(0, 1, 2)?, &[300, 301, 302]);
        assert_eq!(state.values.row(0, 1, 2)?, &[1300, 1301, 1302]);
        Ok(())
    }

    #[test]
    fn sliding_cache_keeps_only_the_last_window_and_tracks_absolute_positions() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Sliding, 1, 1, 2, 3)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let first = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 1,
                seq_len: 2,
                head_dim: 2,
            },
            0,
        )?;
        cache.update_and_fetch(first.view(), first.view())?;

        let second = test_tensor(
            KvTensorShape {
                batch_size: 1,
                kv_head_count: 1,
                seq_len: 2,
                head_dim: 2,
            },
            100,
        )?;
        let state = cache.update_and_fetch(second.view(), second.view())?;

        assert_eq!(state.stored_tokens(), 3);
        assert_eq!(state.start_position(), 1);
        assert_eq!(state.offset(), 4);
        assert_eq!(state.keys.row(0, 0, 0)?, &[10, 11]);
        assert_eq!(state.keys.row(0, 0, 1)?, &[100, 101]);
        assert_eq!(state.keys.row(0, 0, 2)?, &[110, 111]);
        Ok(())
    }

    #[test]
    fn decode_step_reads_match_expected_batch_head_layout() -> Result<()> {
        let spec = GemmaKvCacheSpec::new(GemmaAttentionKind::Full, 2, 2, 2, 4)?;
        let mut cache = GemmaKvCache::<i32>::new(spec)?;

        let decode = test_tensor(
            KvTensorShape {
                batch_size: 2,
                kv_head_count: 2,
                seq_len: 1,
                head_dim: 2,
            },
            500,
        )?;
        let state = cache.update_and_fetch(decode.view(), decode.view())?;

        assert_eq!(state.keys.row(0, 0, 0)?, &[500, 501]);
        assert_eq!(state.keys.row(0, 1, 0)?, &[600, 601]);
        assert_eq!(state.keys.row(1, 0, 0)?, &[1500, 1501]);
        assert_eq!(state.keys.row(1, 1, 0)?, &[1600, 1601]);
        Ok(())
    }

    fn sample_text_config(layer_types: &[&str], num_kv_shared_layers: u32) -> MlxTextConfig {
        MlxTextConfig {
            attention_bias: false,
            attention_dropout: 0.0,
            attention_k_eq_v: false,
            bos_token_id: 2,
            dtype: "bfloat16".to_owned(),
            enable_moe_block: true,
            eos_token_id: 1,
            final_logit_softcapping: 0.0,
            global_head_dim: 4,
            head_dim: 4,
            hidden_activation: "gelu_pytorch_tanh".to_owned(),
            hidden_size: 16,
            hidden_size_per_layer_input: 0,
            initializer_range: 0.02,
            intermediate_size: 32,
            layer_types: layer_types.iter().map(|item| (*item).to_owned()).collect(),
            max_position_embeddings: 16,
            model_type: "gemma4".to_owned(),
            moe_intermediate_size: 32,
            num_attention_heads: 4,
            num_experts: 8,
            num_global_key_value_heads: 1,
            num_hidden_layers: layer_types.len() as u32,
            num_key_value_heads: 2,
            num_kv_shared_layers,
            pad_token_id: 0,
            rms_norm_eps: 1e-6,
            rope_parameters: MlxTextRopeParameters {
                full_attention: MlxRopeAttentionParameters {
                    partial_rotary_factor: None,
                    rope_theta: 10_000.0,
                    rope_type: "default".to_owned(),
                },
                sliding_attention: MlxRopeAttentionParameters {
                    partial_rotary_factor: None,
                    rope_theta: 10_000.0,
                    rope_type: "default".to_owned(),
                },
            },
            sliding_window: 4,
            tie_word_embeddings: true,
            top_k_experts: 2,
            use_bidirectional_attention: "never".to_owned(),
            use_cache: true,
            use_double_wide_mlp: false,
            vocab_size: 256,
            vocab_size_per_layer_input: 0,
        }
    }

    fn test_tensor(shape: KvTensorShape, base: i32) -> Result<KvTensor<i32>> {
        let mut data = Vec::with_capacity(shape.element_count()?);
        for batch in 0..shape.batch_size {
            for head in 0..shape.kv_head_count {
                for token in 0..shape.seq_len {
                    for dim in 0..shape.head_dim {
                        data.push(
                            base + (batch as i32) * 1000
                                + (head as i32) * 100
                                + (token as i32) * 10
                                + dim as i32,
                        );
                    }
                }
            }
        }
        KvTensor::from_vec(shape, data)
    }
}
