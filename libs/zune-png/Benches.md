## Benchmarks for on the library

Okay, let's get this over with.

The benchmarks file are variations of CuteFish OS wallpaper i.e

![img](tests/benchmarks/speed_bench.png)

variations are created by saving different configs in GIMP, (will automate soon), but it works.

### As usual, the disclaimers

Benchmarking may not reflect real world usage.

The author has a tendency of finding one big file,
and decoding it and using that as a metric for optimizations.

This means probably results may be skewed to the author's liking.

But the results are *definitely* skewed to the author's computer.

### Compiler and configuration

Compiler

- rustc 1.68.0-nightly (0f529f0f4 2022-12-13)
- cargo 1.68.0-nightly (70898e522 2022-12-05)

### CPU and OS

- Architecture:          x86_64
- CPU op-mode(s):        32-bit, 64-bit
- Address sizes:         48 bits physical, 48 bits virtual
- Byte Order:            Little Endian
- CPU(s):                 6
- On-line CPU(s) list:   0-5
- Model name:            AMD Ryzen 5 4500U with Radeon Graphics
- CPU family:            23
- CPU max MHz:           2375.0000
- CPU min MHz:           1400.0000

#### Caches (sum of all):

- L1d:                   192 KiB (6 instances)
- L1i:                   192 KiB (6 instances)
- L2:                    3 MiB (6 instances)
- L3:                    8 MiB (2 instances)

## Reproduction steps

Run the following command in your Unix shell of choice

```sh
git clone  https://github.com/etemesi254/zune-image.git
cd ./zune-image/zune-png
RUSTFLAGS='-C target-cpu=native' cargo bench       
```

### Benchmarks on my machine.

| Variation                         | zune-png  | imagers/png | spng      |
|-----------------------------------|-----------|-------------|-----------|
| 8 bytes per pixel                 | 191.15 ms | 296.97 ms   | 251.15ms  |
| 8 bytes per pixel interlaced RGBA | 325.89ms  | 422.87 ms   | 308.48 ms |
