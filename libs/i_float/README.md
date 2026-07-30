# iFloat

This fixed float math library provides an efficient and deterministic solution for arithmetic and geometric operations.

## Features

- Fixed-point arithmetic using FixNumber for deterministic calculations

- 2D fixed-point vector operations with FixVec

- Trigonometric functions using fixed-point angles

- Angle conversion and manipulation utilities in FixAngle


## Installation


```rust
use i_float::fix_float::FixFloat;
use i_float::fix_vec::FixVec;
```


## How It Works

The \`**FixFloat**\` struct uses fixed-point arithmetic to perform calculations with a high degree of precision and determinism. It supports numbers in the range 2^21 - 1 to -2^21 + 1 with a precision of 1/1024, and is most suitable for the range 1,000,000 to -1,000,000 with a precision of 0.01.

Fixed-point numbers are represented using a fixed number of bits for the fractional part. In this implementation, the number of bits representing the fractional part of the fixed-point number is 10, which allows for a precision of 1/1024 or approximately 0.001.

Here are some examples of fixed-point number representation:

- 1 / 1024 ≈ 0.001 (represented as 1)
- 256 / 1024 = 0.25 (represented as 256)
- 1024 / 1024 = 1 (represented as 1024)
- (1024 + 512) / 1024 = 1.5 (represented as 1536)
- (2048 + 256) / 1024 = 2.25 (represented as 2304)

By using the \`**FixFloat**\` struct, you can perform arithmetic operations using i64 values while maintaining the precision of floating-point numbers, ensuring deterministic behavior across different platforms and devices.


## Usage

### FixFloat

The \`**FixFloat**\` struct represents a fixed-point number using a \`**i64**\` as the underlying storage, allowing deterministic arithmetic operations. Use \`**FixFloat**\` for calculations instead of \`**i32**\` or \`**i64**\` when deterministic behavior is required. \`**FixFloat**\` provides a way to perform arithmetic operations with \`**i64**\` values while maintaining the precision of floating-point numbers.

```rust
let a = FixFloat::new_number(1);
let b = FixFloat::new_number(2);
let c = a + b;
```

### FixVec

The \`**FixVec**\` struct represents a 2D fixed-point vector, providing various utility methods and operators for vector operations. Use \`**FixVec**\` for 2D geometric calculations when deterministic behavior is required.

```rust
let a = FixVec::new_number(1, 1);
let b = FixVec::new_number(1, -1);
let c = a + b;
```

### FixAngle
The \`**FixAngle**\` class provides various utility methods for working with fixed-point angles, including trigonometric functions and angle conversion.


```rust
let fix_angle = FixAngle::new_from_radians_f64(angle);
let fix_sin = fix_angle.sin();
let i64_sin = fix_sin.double();
```


## License
