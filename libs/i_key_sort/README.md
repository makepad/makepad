# iKeySort

[![crates.io version](https://img.shields.io/crates/v/i_key_sort.svg)](https://crates.io/crates/i_key_sort)
[![docs.rs docs](https://docs.rs/i_key_sort/badge.svg)](https://docs.rs/i_key_sort)

A fast [Radix-Sort–style](https://en.wikipedia.org/wiki/Radix_sort) algorithm in Rust.  
It combines binning and counting sort ideas: instead of multiple digit passes, it maps
elements directly into power-of-two buckets in a single step, with optional
parallel execution.

Optimized for cases where a primary key can be extracted efficiently to index
elements into buckets.

<img src="https://raw.githubusercontent.com/iShape-Rust/iKeySort/main/readme/sort_algorithm.gif" width="512"/>

## Examples

### 1) Sort by a single key

```rust
use i_key_sort::sort::one_key::OneKeySort;

let mut v = vec![5, 1, 4, 1, 3, 2];

v.sort_by_one_key(true, |&x| x);

assert_eq!(v, [1, 1, 2, 3, 4, 5]);
```

`parallel` is ignored unless feature `allow_multithreading` is enabled.

### 2) Sort by a key, then by a comparator

```rust
use i_key_sort::sort::one_key_cmp::OneKeyAndCmpSort;

let mut v = vec![("b", 2), ("a", 3), ("a", 1)];

v.sort_by_one_key_then_by(true, |x| x.0.as_bytes()[0], |a, b| a.1.cmp(&b.1));

assert_eq!(v, [("a", 1), ("a", 3), ("b", 2)]);
```

### 3) Sort by two keys

```rust
use i_key_sort::sort::two_keys::TwoKeysSort;

let mut v = vec![(2, 1), (1, 2), (1, 0)];

v.sort_by_two_keys(true, |x| x.0, |x| x.1);

assert_eq!(v, [(1, 0), (1, 2), (2, 1)]);
```

### 4) Two keys, then comparator (three-way)

```rust
use i_key_sort::sort::two_keys_cmp::TwoKeysAndCmpSort;

let mut v = vec![(1u32, 0i32, 9i32), (1, 0, 3), (1, 1, 1)];

v.sort_by_two_keys_then_by(true, |x| x.0, |x| x.1, |a, b| a.2.cmp(&b.2));

assert_eq!(v, [(1, 0, 3), (1, 0, 9), (1, 1, 1)]);
```


### 5) Reusing a buffer to avoid allocations

```rust
use i_key_sort::sort::one_key::OneKeySort;

let mut buf = Vec::new();
let mut v = vec![3, 2, 1];

v.sort_by_one_key_and_buffer(true, &mut buf, |&x| x);

assert_eq!(v, [1, 2, 3]);
```
## Benchmarks

### Checkerboard

**Small N (40,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.001274*  | *0.001191*    | *0.000887*      | *0.000622*        | *0.000555* | ***0.000418*** |
| 24 bytes     | *0.001616*  | *0.001722*    | *0.000641*      | *0.000692*        | *0.000647* | ***0.000506*** |
| 40 bytes     | *0.003505*  | *0.002360*    | *0.001156*      | *0.000875*        | *0.000835* | ***0.000743*** |

**Medium N (4,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.218301*  | *0.174744*    | *0.080812*      | *0.061049*        | *0.067390* | ***0.044537*** |
| 24 bytes     | *0.287528*  | *0.240101*    | *0.116495*      | *0.080029*        | *0.088140* | ***0.066674*** |
| 40 bytes     | *0.443824*  | *0.344179*    | *0.192526*      | *0.125253*        | *0.128362* | ***0.107262*** |

**Large N (64,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *4.534396*  | *3.330774*    | *1.567763*      | *1.148979*        | *1.626500* | ***0.826504*** |
| 24 bytes     | *6.122517*  | *4.541138*    | *2.333630*      | *1.552827*        | *2.191277* | ***1.229364*** |
| 40 bytes     | *9.607774*  | *6.718290*    | *3.898845*      | *2.538559*        | *3.354824* | ***2.176637*** |

---

### Random

**Small N (100,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.004663*  | *0.003197*    | *0.001784*      | ***0.001246***    | *0.002610* | *0.001300* |
| 24 bytes     | *0.007669*  | *0.004744*    | *0.002694*      | ***0.001393***    | *0.002492* | *0.002166* |
| 40 bytes     | *0.011109*  | *0.005884*    | *0.002472*      | ***0.001991***    | *0.003643* | *0.002127* |

**Medium N (1,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.058683*  | *0.040153*    | *0.021345*      | *0.012079*        | *0.024275* | ***0.010760*** |
| 24 bytes     | *0.074868*  | *0.054684*    | *0.029665*      | ***0.015684***    | *0.034833* | *0.018664* |
| 40 bytes     | *0.107705*  | *0.078022*    | *0.051723*      | ***0.025710***    | *0.042665* | *0.029814* |

**Large N (10,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.490327*  | *0.603355*    | *0.200557*      | *0.123935*        | *0.244938* | ***0.120322*** |
| 24 bytes     | *0.613799*  | *0.789910*    | *0.310511*      | ***0.182654***    | *0.299847* | *0.189350* |
| 40 bytes     | *1.067161*  | *1.310609*    | *0.500217*      | ***0.294828***    | *0.417369* | *0.308985* |

---

### Spiral

**Small N (100,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.005321*  | *0.003436*    | *0.001438*      | *0.001263*        | *0.001800* | ***0.000973*** |
| 24 bytes     | *0.007175*  | *0.004538*    | *0.001380*      | *0.001377*        | *0.001698* | ***0.001239*** |
| 40 bytes     | *0.011133*  | *0.006619*    | *0.002630*      | *0.002199*        | *0.003734* | ***0.002066*** |

**Medium N (1,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.047821*  | *0.043808*    | *0.019376*      | *0.013097*        | *0.018909* | ***0.010708*** |
| 24 bytes     | *0.063357*  | *0.056806*    | *0.025164*      | ***0.015132***    | *0.024540* | *0.016555* |
| 40 bytes     | *0.097301*  | *0.086633*    | *0.039705*      | ***0.022954***    | *0.034288* | *0.027214* |

**Large N (10,000,000 elements)**

| size         | sort_stable | sort_unstable | par_sort_stable | par_sort_unstable | bin_sort | par_bin_sort |
|--------------|-------------|---------------|-----------------|-------------------|----------|--------------|
| 16 bytes     | *0.578940*  | *0.518973*    | *0.203567*      | *0.141714*        | *0.207762* | ***0.106376*** |
| 24 bytes     | *0.789698*  | *0.708410*    | *0.296379*      | *0.191008*        | *0.258032* | ***0.161334*** |
| 40 bytes     | *1.228367*  | *1.100306*    | *0.484209*      | *0.297303*        | *0.360006* | ***0.261720*** |




