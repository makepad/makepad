//! Backend-neutral model-weight disk layer. One home for every weight
//! format parser in the tree (safetensors, GGUF, torch zip/pickle, npy),
//! the mmap machinery, and the WeightSet API models load through.
//! No GPU dependencies — the cuda/metal stores consume views produced here.
//! Plan of record: /aiarch.md
//!
//! Populated by the restructure lanes: formats/ (safetensors, gguf, torch,
//! npy), mmap.rs, weight_set.rs.
