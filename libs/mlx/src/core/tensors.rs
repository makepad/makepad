#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MlxDType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    BF16,
    F32,
    F64,
}

impl MlxDType {
    pub fn from_safetensors_str(value: &str) -> Result<Self> {
        match value {
            "BOOL" => Ok(Self::Bool),
            "U8" => Ok(Self::U8),
            "U16" => Ok(Self::U16),
            "U32" => Ok(Self::U32),
            "U64" => Ok(Self::U64),
            "I8" => Ok(Self::I8),
            "I16" => Ok(Self::I16),
            "I32" => Ok(Self::I32),
            "I64" => Ok(Self::I64),
            "F16" => Ok(Self::F16),
            "BF16" => Ok(Self::BF16),
            "F32" => Ok(Self::F32),
            "F64" => Ok(Self::F64),
            other => Err(MlxRtError::InvalidSafetensors {
                path: PathBuf::new(),
                message: format!("unsupported dtype {}", other),
            }),
        }
    }

    pub fn byte_width(self) -> u64 {
        match self {
            Self::Bool | Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 | Self::F16 | Self::BF16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::U64 | Self::I64 | Self::F64 => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxTensorEntry {
    pub dtype: MlxDType,
    pub shape: Vec<u64>,
    pub data_offsets: [u64; 2],
}

impl MlxTensorEntry {
    pub fn element_count(&self) -> u64 {
        self.shape.iter().copied().product::<u64>()
    }

    pub fn data_len_bytes(&self) -> u64 {
        self.data_offsets[1] - self.data_offsets[0]
    }

    pub fn expected_len_bytes(&self) -> u64 {
        self.element_count() * self.dtype.byte_width()
    }

    pub fn file_offsets(&self, payload_base_offset: u64) -> [u64; 2] {
        [
            payload_base_offset + self.data_offsets[0],
            payload_base_offset + self.data_offsets[1],
        ]
    }
}

#[derive(Clone, Debug)]
pub struct MlxSafetensorsHeader {
    pub path: PathBuf,
    pub file_len: u64,
    pub header_len: u64,
    pub metadata: HashMap<String, String>,
    pub tensors: HashMap<String, MlxTensorEntry>,
    file: Arc<Mutex<fs::File>>,
}

impl MlxSafetensorsHeader {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = fs::File::open(&path).map_err(|err| MlxRtError::Io {
            path: path.clone(),
            message: err.to_string(),
        })?;
        let file_len = file
            .metadata()
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?
            .len();

        let mut header_len_bytes = [0u8; 8];
        file.read_exact(&mut header_len_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_len = u64::from_le_bytes(header_len_bytes);
        let payload_base_offset =
            8u64.checked_add(header_len)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: "header length overflow".to_string(),
                })?;
        if payload_base_offset > file_len {
            return Err(MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: format!(
                    "header extends past EOF: payload base {} > file len {}",
                    payload_base_offset, file_len
                ),
            });
        }

        let mut header_bytes = vec![0u8; header_len as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|err| MlxRtError::Io {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_text =
            String::from_utf8(header_bytes).map_err(|err| MlxRtError::InvalidSafetensors {
                path: path.clone(),
                message: err.to_string(),
            })?;
        let header_map =
            HashMap::<String, JsonValue>::deserialize_json(&header_text).map_err(|err| {
                MlxRtError::Json {
                    path: path.clone(),
                    message: format!("{:?}", err),
                }
            })?;

        let mut metadata = HashMap::new();
        let mut tensors = HashMap::new();

        for (name, value) in header_map {
            if name == "__metadata__" {
                metadata = json_string_map(&path, "__metadata__", &value)?;
                continue;
            }
            let object = json_object(&path, &name, &value)?;
            let dtype = json_dtype(&path, &name, object.get("dtype"))?;
            let shape = json_u64_array(&path, &name, object.get("shape"))?;
            let data_offsets = json_two_u64s(&path, &name, object.get("data_offsets"))?;
            let entry = MlxTensorEntry {
                dtype,
                shape,
                data_offsets,
            };
            let file_offsets = entry.file_offsets(payload_base_offset);
            if file_offsets[1] > file_len {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} ends past EOF: {} > {}",
                        name, file_offsets[1], file_len
                    ),
                });
            }
            if entry.data_len_bytes() != entry.expected_len_bytes() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: path.clone(),
                    message: format!(
                        "tensor {} length mismatch: stored {} expected {}",
                        name,
                        entry.data_len_bytes(),
                        entry.expected_len_bytes()
                    ),
                });
            }
            tensors.insert(name, entry);
        }

        Ok(Self {
            path,
            file_len,
            header_len,
            metadata,
            tensors,
            file: Arc::new(Mutex::new(file)),
        })
    }

    pub fn payload_base_offset(&self) -> u64 {
        8 + self.header_len
    }

    pub fn tensor(&self, name: &str) -> Option<&MlxTensorEntry> {
        self.tensors.get(name)
    }

    fn read_file_range(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        let mut file = self.file.lock().map_err(|_| MlxRtError::Io {
            path: self.path.clone(),
            message: "safetensors file mutex poisoned".to_string(),
        })?;
        file.seek(SeekFrom::Start(start))
            .map_err(|err| MlxRtError::Io {
                path: self.path.clone(),
                message: err.to_string(),
            })?;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        Ok(bytes)
    }

    pub fn read_tensor_bytes(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        self.read_file_range(file_offsets[0], entry.data_len_bytes() as usize)
    }

    pub fn read_rank2_row_bytes(&self, name: &str, row: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 2 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 2, got {:?}", name, entry.shape),
            });
        }
        if row >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} out of range", name, row),
            });
        }
        let row_bytes = entry.shape[1] * entry.dtype.byte_width();
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0] + row * row_bytes;
        let end = start + row_bytes;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} row {} extends past tensor payload", name, row),
            });
        }
        self.read_file_range(start, row_bytes as usize)
    }

    pub fn read_rank2_row_u32_words(&self, name: &str, row: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank2_row_bf16_words(&self, name: &str, row: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank2_row_bytes(name, row)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    fn read_rank3_plane_bytes(&self, name: &str, plane: u64) -> Result<Vec<u8>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.shape.len() != 3 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected rank 3, got {:?}", name, entry.shape),
            });
        }
        if plane >= entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane {} out of range", name, plane),
            });
        }
        let plane_elems = entry.shape[1].checked_mul(entry.shape[2]).ok_or_else(|| {
            MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane element count overflow", name),
            }
        })?;
        let plane_bytes = plane_elems
            .checked_mul(entry.dtype.byte_width())
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane byte count overflow", name),
            })?;
        let file_offsets = entry.file_offsets(self.payload_base_offset());
        let start = file_offsets[0]
            .checked_add(plane.checked_mul(plane_bytes).ok_or_else(|| {
                MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} plane offset overflow", name),
                }
            })?)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane start overflow", name),
            })?;
        let end = start
            .checked_add(plane_bytes)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} plane end overflow", name),
            })?;
        if end > file_offsets[1] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} plane {} extends past tensor payload",
                    name, plane
                ),
            });
        }
        self.read_file_range(start, plane_bytes as usize)
    }

    pub fn read_rank3_plane_u32_words(&self, name: &str, plane: u64) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_rank3_plane_bf16_words(&self, name: &str, plane: u64) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_rank3_plane_bytes(name, plane)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    pub fn read_u32_tensor_words(&self, name: &str) -> Result<Vec<u32>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected U32, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 4);
        for chunk in bytes.chunks_exact(4) {
            out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(out)
    }

    pub fn read_bf16_tensor_words(&self, name: &str) -> Result<Vec<u16>> {
        let entry = self
            .tensor(name)
            .ok_or_else(|| MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} not found in header", name),
            })?;
        if entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("tensor {} expected BF16, got {:?}", name, entry.dtype),
            });
        }
        let bytes = self.read_tensor_bytes(name)?;
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            out.push(u16::from_le_bytes([chunk[0], chunk[1]]));
        }
        Ok(out)
    }

    pub fn affine_dequantize_row_f32(
        &self,
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        row: u64,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine dequant bits {}", bits),
            });
        }
        let weight = self.read_rank2_row_u32_words(weight_name, row)?;
        let scales = self.read_rank2_row_bf16_words(scales_name, row)?;
        let biases = self.read_rank2_row_bf16_words(biases_name, row)?;
        if scales.len() != biases.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "row {} scale/bias length mismatch: {} vs {}",
                    row,
                    scales.len(),
                    biases.len()
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let out_size = weight.len() as u64 * values_per_word;
        if out_size != scales.len() as u64 * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "row {} packed/scales shape mismatch for group_size={} bits={}",
                    row, group_size, bits
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight.len() as u64 != scales.len() as u64 * words_per_group {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("row {} invalid words_per_group {}", row, words_per_group),
            });
        }
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(out_size as usize);
        for group_idx in 0..scales.len() {
            let scale = bf16_word_to_f32(scales[group_idx]);
            let bias = bf16_word_to_f32(biases[group_idx]);
            let group_start = group_idx * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            for packed in &weight[group_start..group_end] {
                for shift in (0..32).step_by(bits as usize) {
                    let q = ((*packed >> shift) & mask) as f32;
                    out.push(bf16_round_to_f32(q * scale + bias));
                }
            }
        }
        Ok(out)
    }

    pub fn affine_quantized_matmul_t_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 2
            || scales_entry.shape.len() != 2
            || biases_entry.shape.len() != 2
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[1] * values_per_word;
        if inner_dim != scales_entry.shape[1] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weights = self.read_u32_tensor_words(weight_name)?;
        let scales = self.read_bf16_tensor_words(scales_name)?;
        let biases = self.read_bf16_tensor_words(biases_name)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let rows = weight_entry.shape[0] as usize;
        let weight_stride = weight_entry.shape[1] as usize;
        let groups_per_row = scales_entry.shape[1] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(rows);

        for row in 0..rows {
            let weight_row_start = row * weight_stride;
            let qparam_row_start = row * groups_per_row;
            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
                let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
                let group_start = weight_row_start + group * words_per_group as usize;
                let group_end = group_start + words_per_group as usize;
                for packed in &weights[group_start..group_end] {
                    let mut packed_word = *packed;
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }
            out.push(sum);
        }

        Ok(out)
    }

    pub fn affine_quantized_matmul_t_top1_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        group_size: u64,
        bits: u32,
        softcap: Option<f32>,
    ) -> Result<MlxGreedyToken> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 2
            || scales_entry.shape.len() != 2
            || biases_entry.shape.len() != 2
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[1] * values_per_word;
        if inner_dim != scales_entry.shape[1] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weight_offsets = weight_entry.file_offsets(self.payload_base_offset());
        let scales_offsets = scales_entry.file_offsets(self.payload_base_offset());
        let biases_offsets = biases_entry.file_offsets(self.payload_base_offset());
        let weight_row_bytes = (weight_entry.shape[1] * weight_entry.dtype.byte_width()) as usize;
        let qparam_row_bytes = (scales_entry.shape[1] * scales_entry.dtype.byte_width()) as usize;
        let rows = weight_entry.shape[0] as usize;
        let groups_per_row = scales_entry.shape[1] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let mut weight_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut scales_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut biases_file = fs::File::open(&self.path).map_err(|err| MlxRtError::Io {
            path: self.path.clone(),
            message: err.to_string(),
        })?;
        let mut weight_bytes = vec![0u8; weight_row_bytes];
        let mut scales_bytes = vec![0u8; qparam_row_bytes];
        let mut biases_bytes = vec![0u8; qparam_row_bytes];

        let mut best = MlxGreedyToken {
            token_id: 0,
            logit: f32::NEG_INFINITY,
        };
        for row in 0..rows {
            let weight_row_offset = weight_offsets[0] + row as u64 * weight_row_bytes as u64;
            let qparam_row_offset = scales_offsets[0] + row as u64 * qparam_row_bytes as u64;
            let bias_row_offset = biases_offsets[0] + row as u64 * qparam_row_bytes as u64;
            weight_file
                .seek(SeekFrom::Start(weight_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            weight_file
                .read_exact(&mut weight_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            scales_file
                .seek(SeekFrom::Start(qparam_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            scales_file
                .read_exact(&mut scales_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            biases_file
                .seek(SeekFrom::Start(bias_row_offset))
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;
            biases_file
                .read_exact(&mut biases_bytes)
                .map_err(|err| MlxRtError::Io {
                    path: self.path.clone(),
                    message: err.to_string(),
                })?;

            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale_byte_offset = group * 2;
                let scale = bf16_word_to_f32(u16::from_le_bytes([
                    scales_bytes[scale_byte_offset],
                    scales_bytes[scale_byte_offset + 1],
                ]));
                let bias = bf16_word_to_f32(u16::from_le_bytes([
                    biases_bytes[scale_byte_offset],
                    biases_bytes[scale_byte_offset + 1],
                ]));
                let group_start = group * words_per_group as usize * 4;
                let group_end = group_start + words_per_group as usize * 4;
                for chunk in weight_bytes[group_start..group_end].chunks_exact(4) {
                    let mut packed_word =
                        u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }

            let logit = if let Some(softcap) = softcap.filter(|softcap| *softcap > 0.0) {
                bf16_round_to_f32((sum / softcap).tanh() * softcap)
            } else {
                sum
            };
            if logit > best.logit {
                best = MlxGreedyToken {
                    token_id: row as u32,
                    logit,
                };
            }
        }

        Ok(best)
    }

    pub fn affine_quantized_matmul_t_f32_rank3_plane(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        scales_name: &str,
        biases_name: &str,
        plane: u64,
        group_size: u64,
        bits: u32,
    ) -> Result<Vec<f32>> {
        if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("unsupported affine quantized matmul bits {}", bits),
            });
        }
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", weight_name),
                })?;
        let scales_entry =
            self.tensor(scales_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", scales_name),
                })?;
        let biases_entry =
            self.tensor(biases_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("tensor {} not found in header", biases_name),
                })?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensor {} expected U32, got {:?}",
                    weight_name, weight_entry.dtype
                ),
            });
        }
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "tensors {} / {} expected BF16, got {:?} / {:?}",
                    scales_name, biases_name, scales_entry.dtype, biases_entry.dtype
                ),
            });
        }
        if weight_entry.shape.len() != 3
            || scales_entry.shape.len() != 3
            || biases_entry.shape.len() != 3
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rank-3 affine quantized matmul expects rank-3 tensors, got {:?} {:?} {:?}",
                    weight_entry.shape, scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if scales_entry.shape != biases_entry.shape {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "scale/bias shape mismatch: {:?} vs {:?}",
                    scales_entry.shape, biases_entry.shape
                ),
            });
        }
        if weight_entry.shape[0] != scales_entry.shape[0]
            || weight_entry.shape[1] != scales_entry.shape[1]
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "weight/scales outer shape mismatch: {:?} vs {:?}",
                    weight_entry.shape, scales_entry.shape
                ),
            });
        }
        if plane >= weight_entry.shape[0] {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "plane {} out of range for tensor {} with {} planes",
                    plane, weight_name, weight_entry.shape[0]
                ),
            });
        }
        let values_per_word = 32 / bits as u64;
        let inner_dim = weight_entry.shape[2] * values_per_word;
        if inner_dim != scales_entry.shape[2] * group_size {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "packed/scales plane shape mismatch for group_size={} bits={}",
                    group_size, bits
                ),
            });
        }
        if x_bf16_words.len() as u64 != inner_dim {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    inner_dim
                ),
            });
        }
        let words_per_group = group_size / values_per_word;
        if words_per_group == 0 || weight_entry.shape[2] != scales_entry.shape[2] * words_per_group
        {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!("invalid words_per_group {}", words_per_group),
            });
        }

        let weights = self.read_rank3_plane_u32_words(weight_name, plane)?;
        let scales = self.read_rank3_plane_bf16_words(scales_name, plane)?;
        let biases = self.read_rank3_plane_bf16_words(biases_name, plane)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let rows = weight_entry.shape[1] as usize;
        let weight_stride = weight_entry.shape[2] as usize;
        let groups_per_row = scales_entry.shape[2] as usize;
        let pack_factor = values_per_word as usize;
        let mask = (1u32 << bits) - 1;
        let mut out = Vec::with_capacity(rows);

        for row in 0..rows {
            let weight_row_start = row * weight_stride;
            let qparam_row_start = row * groups_per_row;
            let mut sum = 0.0f32;
            let mut x_index = 0usize;
            for group in 0..groups_per_row {
                let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
                let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
                let group_start = weight_row_start + group * words_per_group as usize;
                let group_end = group_start + words_per_group as usize;
                for packed in &weights[group_start..group_end] {
                    let mut packed_word = *packed;
                    for _ in 0..pack_factor {
                        let q = (packed_word & mask) as f32;
                        let deq_mul = bf16_round_to_f32(scale * q);
                        let deq = bf16_round_to_f32(deq_mul + bias);
                        let prod = bf16_round_to_f32(x[x_index] * deq);
                        sum = bf16_round_to_f32(sum + prod);
                        x_index += 1;
                        if bits != 8 {
                            packed_word >>= bits;
                        }
                    }
                }
            }
            out.push(sum);
        }

        Ok(out)
    }

    pub fn rms_norm_weighted_f32(
        &self,
        x_bf16_words: &[u16],
        weight_name: &str,
        eps: f32,
    ) -> Result<Vec<f32>> {
        let weight_entry =
            self.tensor(weight_name)
                .ok_or_else(|| MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!("missing tensor {}", weight_name),
                })?;
        if weight_entry.shape.len() != 1 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rms_norm expects rank-1 weight, got {:?}",
                    weight_entry.shape
                ),
            });
        }
        let hidden = weight_entry.shape[0] as usize;
        if x_bf16_words.len() != hidden {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "rms_norm activation length mismatch: got {} expected {}",
                    x_bf16_words.len(),
                    hidden
                ),
            });
        }

        let weight_words = self.read_bf16_tensor_words(weight_name)?;
        let x = x_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();

        let mut mean_square = 0.0f32;
        for value in &x {
            mean_square += value * value;
        }
        mean_square /= hidden as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

        let mut out = Vec::with_capacity(hidden);
        for (index, value) in x.iter().copied().enumerate() {
            let normalized = bf16_round_to_f32(value * inv_rms);
            let weight = bf16_word_to_f32(weight_words[index]);
            out.push(bf16_round_to_f32(normalized * weight));
        }
        Ok(out)
    }

    pub fn gemma_router_topk_from_residual_bf16(
        &self,
        residual_bf16_words: &[u16],
        router_scale_name: &str,
        per_expert_scale_name: &str,
        proj_weight_name: &str,
        proj_scales_name: &str,
        proj_biases_name: &str,
        eps: f32,
        top_k: usize,
    ) -> Result<MlxRouterTopKOutput> {
        if top_k == 0 {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "router top_k must be greater than zero".to_string(),
            });
        }

        let hidden = residual_bf16_words.len();
        let router_scale_words = self.read_bf16_tensor_words(router_scale_name)?;
        if router_scale_words.len() != hidden {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router scale length mismatch: got {} expected {}",
                    router_scale_words.len(),
                    hidden
                ),
            });
        }
        let per_expert_scale_words = self.read_bf16_tensor_words(per_expert_scale_name)?;
        if top_k > per_expert_scale_words.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router top_k {} exceeds num_experts {}",
                    top_k,
                    per_expert_scale_words.len()
                ),
            });
        }

        let residual = residual_bf16_words
            .iter()
            .copied()
            .map(bf16_word_to_f32)
            .collect::<Vec<_>>();
        let mut mean_square = 0.0f32;
        for value in &residual {
            mean_square += value * value;
        }
        mean_square /= hidden as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

        let root_size = bf16_round_to_f32((hidden as f32).powf(-0.5));
        let mut router_scaled = Vec::with_capacity(hidden);
        let mut router_scaled_words = Vec::with_capacity(hidden);
        for (index, value) in residual.iter().copied().enumerate() {
            let normed = bf16_round_to_f32(value * inv_rms);
            let scaled_root = bf16_round_to_f32(normed * root_size);
            let scaled =
                bf16_round_to_f32(scaled_root * bf16_word_to_f32(router_scale_words[index]));
            router_scaled.push(scaled);
            router_scaled_words.push(f32_to_bf16_word(scaled));
        }

        let expert_scores = self.affine_quantized_matmul_t_f32(
            &router_scaled_words,
            proj_weight_name,
            proj_scales_name,
            proj_biases_name,
            64,
            4,
        )?;
        if expert_scores.is_empty() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "router projection produced no scores".to_string(),
            });
        }
        if top_k > expert_scores.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "router top_k {} exceeds expert_scores length {}",
                    top_k,
                    expert_scores.len()
                ),
            });
        }

        let max_score = expert_scores
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = expert_scores
            .iter()
            .copied()
            .map(|value| (value - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        let router_probs = exp_scores
            .iter()
            .copied()
            .map(|value| bf16_round_to_f32(value / exp_sum))
            .collect::<Vec<_>>();

        let mut indices = (0..expert_scores.len()).collect::<Vec<_>>();
        indices.sort_by(|&lhs, &rhs| {
            expert_scores[rhs]
                .total_cmp(&expert_scores[lhs])
                .then_with(|| lhs.cmp(&rhs))
        });
        let top_k_indices = indices
            .into_iter()
            .take(top_k)
            .map(|index| index as u32)
            .collect::<Vec<_>>();

        let mut top_k_weights = top_k_indices
            .iter()
            .copied()
            .map(|index| router_probs[index as usize])
            .collect::<Vec<_>>();
        let mut top_k_sum = 0.0f32;
        for weight in &top_k_weights {
            top_k_sum = bf16_round_to_f32(top_k_sum + *weight);
        }
        for (slot, weight) in top_k_weights.iter_mut().enumerate() {
            let normalized = bf16_round_to_f32(*weight / top_k_sum);
            let expert_scale =
                bf16_word_to_f32(per_expert_scale_words[top_k_indices[slot] as usize]);
            *weight = bf16_round_to_f32(normalized * expert_scale);
        }

        Ok(MlxRouterTopKOutput {
            router_scaled,
            expert_scores,
            router_probs,
            top_k_indices,
            top_k_weights,
        })
    }

    pub fn gemma_moe_expert_block_from_residual_bf16(
        &self,
        residual_bf16_words: &[u16],
        pre_feedforward_norm2_weight_name: &str,
        expert_gate_weight_name: &str,
        expert_gate_scales_name: &str,
        expert_gate_biases_name: &str,
        expert_up_weight_name: &str,
        expert_up_scales_name: &str,
        expert_up_biases_name: &str,
        expert_down_weight_name: &str,
        expert_down_scales_name: &str,
        expert_down_biases_name: &str,
        top_k_indices: &[u32],
        top_k_weights: &[f32],
        eps: f32,
    ) -> Result<MlxGemmaMoeExpertOutput> {
        if top_k_indices.is_empty() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: "moe expert path needs at least one routed expert".to_string(),
            });
        }
        if top_k_indices.len() != top_k_weights.len() {
            return Err(MlxRtError::InvalidSafetensors {
                path: self.path.clone(),
                message: format!(
                    "top_k index/weight length mismatch: {} vs {}",
                    top_k_indices.len(),
                    top_k_weights.len()
                ),
            });
        }

        let pre_feedforward_norm2 = self.rms_norm_weighted_f32(
            residual_bf16_words,
            pre_feedforward_norm2_weight_name,
            eps,
        )?;
        let pre_feedforward_norm2_words = pre_feedforward_norm2
            .iter()
            .copied()
            .map(f32_to_bf16_word)
            .collect::<Vec<_>>();

        let mut gate_proj = Vec::new();
        let mut up_proj = Vec::new();
        let mut geglu = Vec::new();
        let mut down_proj = Vec::new();
        let hidden = residual_bf16_words.len();

        for &expert_index in top_k_indices {
            let gate_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &pre_feedforward_norm2_words,
                expert_gate_weight_name,
                expert_gate_scales_name,
                expert_gate_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            let up_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &pre_feedforward_norm2_words,
                expert_up_weight_name,
                expert_up_scales_name,
                expert_up_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            if gate_row.len() != up_row.len() {
                return Err(MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!(
                        "expert {} gate/up output length mismatch: {} vs {}",
                        expert_index,
                        gate_row.len(),
                        up_row.len()
                    ),
                });
            }
            let mut geglu_row = Vec::with_capacity(gate_row.len());
            for (&gate, &up) in gate_row.iter().zip(up_row.iter()) {
                let gate_sq = bf16_round_to_f32(gate * gate);
                let gate_cubic = bf16_round_to_f32(gate_sq * gate);
                let gate_poly =
                    bf16_round_to_f32(gate + bf16_round_to_f32(0.044_715f32 * gate_cubic));
                let gate_tanh_input = bf16_round_to_f32(0.797_884_6f32 * gate_poly);
                let gate_tanh = bf16_round_to_f32(gate_tanh_input.tanh());
                let gate_one_plus = bf16_round_to_f32(1.0f32 + gate_tanh);
                let gate_half = bf16_round_to_f32(0.5f32 * gate);
                let gate_gelu = bf16_round_to_f32(gate_half * gate_one_plus);
                geglu_row.push(bf16_round_to_f32(gate_gelu * up));
            }
            let geglu_words = geglu_row
                .iter()
                .copied()
                .map(f32_to_bf16_word)
                .collect::<Vec<_>>();
            let down_row = self.affine_quantized_matmul_t_f32_rank3_plane(
                &geglu_words,
                expert_down_weight_name,
                expert_down_scales_name,
                expert_down_biases_name,
                expert_index as u64,
                64,
                4,
            )?;
            if down_row.len() != hidden {
                return Err(MlxRtError::InvalidSafetensors {
                    path: self.path.clone(),
                    message: format!(
                        "expert {} down projection length mismatch: got {} expected {}",
                        expert_index,
                        down_row.len(),
                        hidden
                    ),
                });
            }
            gate_proj.extend_from_slice(&gate_row);
            up_proj.extend_from_slice(&up_row);
            geglu.extend_from_slice(&geglu_row);
            down_proj.extend_from_slice(&down_row);
        }

        let mut expert_out = vec![0.0f32; hidden];
        for (expert_slot, &weight) in top_k_weights.iter().enumerate() {
            for hidden_index in 0..hidden {
                let weighted =
                    bf16_round_to_f32(down_proj[expert_slot * hidden + hidden_index] * weight);
                expert_out[hidden_index] = bf16_round_to_f32(expert_out[hidden_index] + weighted);
            }
        }

        Ok(MlxGemmaMoeExpertOutput {
            pre_feedforward_norm2,
            gate_proj,
            up_proj,
            geglu,
            down_proj,
            expert_out,
        })
    }
}

pub const GEMMA4_QPROJ_CASE_INNER_DIM: usize = 2_816;
pub const GEMMA4_QPROJ_CASE_OUTPUT_DIM: usize = 4_096;
pub const GEMMA4_QPROJ_CASE_OUTPUT_FNV1A64: u64 = 0x4A22_9C27_44EA_03B8;
pub const GEMMA4_QPROJ_CASE_ACTIVATION_PATTERN: [f32; 16] = [
    -1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75, 1.0, 0.5, 0.0, -0.5, -1.0, 0.125, 0.375, 0.625,
];
