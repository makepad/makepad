# rustybuzz
![Build Status](https://github.com/RazrFalcon/rustybuzz/workflows/Rust/badge.svg)
[![Crates.io](https://img.shields.io/crates/v/rustybuzz.svg)](https://crates.io/crates/rustybuzz)
[![Documentation](https://docs.rs/rustybuzz/badge.svg)](https://docs.rs/rustybuzz)

`rustybuzz` is a complete [harfbuzz](https://github.com/harfbuzz/harfbuzz)'s
shaping algorithm port to Rust.

Matches `harfbuzz` v2.7.1

## Why?

Because you can add `rustybuzz = "*"` to your project and it just works.
No need for a C++ compiler. No need to configure anything. No need to link to system libraries.

## Conformance

rustybuzz passes 98% of harfbuzz tests (1764 to be more precise).
So it's mostly identical, but there are still some tiny edge-cases which
are not implemented yet or cannot be implemented at all.

Also, Apple layout is largely untested, because we cannot include Apple fonts for legal reasons.
harfbuzz uses macOS CI instances to test it, which is extremely painful
and we do not do this for now.

## Major changes

- Subsetting removed.
- TrueType parsing is completely handled by the
  [ttf-parser](https://github.com/RazrFalcon/ttf-parser).
  And while the parsing algorithm is very different, it's not better or worse, just different.
- Malformed fonts will cause an error. HarfBuzz uses fallback/dummy shaper in this case.
- No font size property. Shaping is always using UnitsPerEm. You should scale the result manually.
- Most of the TrueType and Unicode handling code was moved into separate crates.
- rustybuzz doesn't interact with any system libraries and must produce exactly the same
  results on all OS'es and targets.
- `mort` table is not supported, since it's deprecated by Apple.
- No Arabic fallback shaper, since it requires subsetting.
- No `graphite` library support.
- No automated Apple layout testing for legal reasons. We just cannot include Apple fonts.
  harfbuzz avoids this by running such tests only on CI, which is far from ideal.

## Performance

At the moment, performance isn't that great. We're 1.5-2x slower than harfbuzz.
Also, rustybuzz doesn't support shaping plan caching at the moment.

See [benches/README.md](./benches/README.md) for details.

## Notes about the port

rustybuzz is not a faithful port.

harfbuzz can roughly be split into 6 parts: shaping, subsetting, TrueType parsing,
Unicode routines, custom containers and utilities (harfbuzz doesn't use C++ std)
and glue for system/3rd party libraries. In the mean time, rustybuzz contains only shaping.
All of the TrueType parsing was moved to the [ttf-parser](https://github.com/RazrFalcon/ttf-parser).
Subsetting was removed. Unicode code was mostly moved to external crates.
We don't need custom containers because Rust's std is good enough.
And we do not use any non Rust libraries, so no glue code either.

In the end, we still have around 20 KLOC. While harfbuzz is around 80 KLOC.

## Lines of code

As mentioned above, rustybuzz has around 20 KLOC. But this is not strictly true,
because there are a lot of auto-generated data tables.

You can find the "real" code size using:

```sh
tokei --exclude unicode_norm.rs --exclude complex/vowel_constraints.rs \
      --exclude '*_machine.rs' --exclude '*_table.rs' src
```

Which gives us around 13 KLOC, which is still a lot.

## Future work

Since the port is finished, there is not much to do other than syncing it with
a new harfbuzz releases.
But there are still a lot of room for performance optimizations and refactoring.

Also, despite the fact that harfbuzz has a vast test suite, there are still a lot of
things left to test.

## Safety

The library is completely safe.

We do have one `unsafe` to cast between two POD structures, which is perfectly safe.
But except that, there are no `unsafe` in this library and in most of its dependencies
(excluding `bytemuck`).

## Alternatives

- [harfbuzz_rs](https://crates.io/crates/harfbuzz_rs) - bindings to the actual harfbuzz library.
  As of v2 doesn't expose subsetting and glyph outlining, which harfbuzz supports.
- [allsorts](https://github.com/yeslogic/allsorts) - shaper and subsetter.
  As of v0.6 doesn't support variable fonts and
  [Apple Advanced Typography](https://developer.apple.com/fonts/TrueType-Reference-Manual/RM06/Chap6AATIntro.html).
  Relies on some unsafe code.
- [swash](https://github.com/dfrg/swash) - Supports variable fonts, text layout and rendering.
  No subsetting. Relies on some unsafe code. As of v0.1.4 has zero tests.

## License

`rustybuzz` is licensed under the **MIT**.

`harfbuzz` is [licensed](https://github.com/harfbuzz/harfbuzz/blob/master/COPYING) under the **Old MIT**
