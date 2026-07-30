//! # i_key_sort
//!
//! Cache-friendly, allocation-aware bin/counting style sort with optional parallel
//! pre-spread. Designed for integer-like keys (u8…u64, i8…i64, usize) and for
//! workloads where keys are **bounded** and often **repeat** (so bins help).
//!
//! ## What this crate provides
//! - **Stable API, unsafe-free at the call site**: internals use `unsafe` for speed, public APIs are safe.
//! - **no_std by default**: works with `alloc`. Enable `std`/parallel via features.
//! - **Single-pass bin pre-spread** + **in-bin sort** (key-only, key+cmp, two keys, two keys+cmp).
//! - **Optional parallel pre-spread** using Rayon (feature-gated).
//! - **Reusable buffer** variants to avoid repeated allocations.
//!
//! ## When it shines
//! - Keys are integers (or mapped to them) with a relatively small range.
//! - Many equal keys (histogram is spiky).
//! - Large inputs benefit from linear pre-spread and cache-friendly copying.
//!
//! ## Crate features
//! - `std` — opt in to the Rust standard library (still `no_std` by default).
//! - `allow_multithreading` — enables parallel pre-spread via Rayon and exposes `parallel: bool` paths.
//!   - When this feature is **off**, the `parallel` argument is accepted but **ignored**.
//!
//! ## Complexity (high level)
//! - Pre-spread: `O(n)` with one pass over the input.
//! - In-bin sort: small bins use `sort_unstable` (fast for tiny slices).
//!   Worst-case behaves like `O(n log n)` if all items fall into one large bin,
//!   but typical cases approach linear time when bins distribute well.
//!
//! ## Safety & preconditions
//! - Public APIs are safe; internal `unsafe` is encapsulated and documented.
//! - Your key functions must be **total** and **consistent** (same input → same key).
//! - For signed keys, the internal arithmetic assumes `value >= min_key` during binning;
//!   this is enforced in debug builds and maintained by construction.
//!
//! ## Examples
//!
//! ### 1) Sort by a single key
//! ```rust
//! use i_key_sort::sort::one_key::OneKeySort;
//!
//! let mut v = vec![5, 1, 4, 1, 3, 2];
//! // `parallel` is ignored unless feature `allow_multithreading` is enabled.
//! v.sort_by_one_key(/* parallel: */ true, |&x| x);
//!
//! assert_eq!(v, [1, 1, 2, 3, 4, 5]);
//! ```
//!
//! ### 2) Sort by a key, then by a comparator
//! ```rust
//! use i_key_sort::sort::one_key_cmp::OneKeyAndCmpSort;
//!
//! let mut v = vec![("b", 2), ("a", 3), ("a", 1)];
//! v.sort_by_one_key_then_by(true, |x| x.0.as_bytes()[0], |a, b| a.1.cmp(&b.1));
//!
//! assert_eq!(v, [("a", 1), ("a", 3), ("b", 2)]);
//! ```
//!
//! ### 3) Sort by two keys (lexicographic)
//! ```rust
//! use i_key_sort::sort::two_keys::TwoKeysSort;
//!
//! let mut v = vec![(2, 1), (1, 2), (1, 0)];
//! v.sort_by_two_keys(true, |x| x.0, |x| x.1);
//!
//! assert_eq!(v, [(1, 0), (1, 2), (2, 1)]);
//! ```
//!
//! ### 4) Two keys, then comparator (three-way)
//! ```rust
//! use i_key_sort::sort::two_keys_cmp::TwoKeysAndCmpSort;
//!
//! let mut v = vec![(1u32, 0i32, 9i32), (1, 0, 3), (1, 1, 1)];
//! v.sort_by_two_keys_then_by(true, |x| x.0, |x| x.1, |a, b| a.2.cmp(&b.2));
//!
//! assert_eq!(v, [(1, 0, 3), (1, 0, 9), (1, 1, 1)]);
//! ```
//!
//! ### 5) Reusing a buffer to avoid allocations
//! ```rust
//! use i_key_sort::sort::one_key::OneKeySort;
//!
//! let mut buf = Vec::new();
//! let mut v = vec![3, 2, 1];
//!
//! v.sort_by_one_key_and_buffer(true, &mut buf, |&x| x);
//!
//! assert_eq!(v, [1, 2, 3]);
//! ```
//!
//! ## no_std notes
//! The crate is `no_std` by default and pulls `alloc`. Enable the `std` feature
//! (or `allow_multithreading`, which implies `std`) if you need threading or
//! want to use Rayon-powered paths.
//!
//! ## License
//! ## MIT

#![cfg_attr(not(feature = "std"), no_std)]
pub mod sort;

extern crate alloc;
