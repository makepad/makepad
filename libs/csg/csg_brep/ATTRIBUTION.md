# Attribution

solidcore is a Rust port of **SMLib**, the NURBS B-rep solid modeling kernel by
NVIDIA, published as part of USD BRep:

- Repository: <https://github.com/NVIDIA-Omniverse/usd-brep>
- Pinned release: v1.0.1, commit `3fd5b95` (`scripts/upstream-pin.env`; ported from
  `c9c5979` up to the move to v1.0.1)
- Licence: Apache License, Version 2.0 (see [`LICENSE`](LICENSE), copied verbatim
  from upstream, and [`NOTICE`](NOTICE))
- Copyright: (c) 2020-2026 NVIDIA CORPORATION & AFFILIATES

solidcore is an independent project. It is not affiliated with or endorsed by
NVIDIA, and "SMLib" and "NVIDIA" are used here only to identify the origin of the
code, not as branding.

## Upstream material used

Only these upstream paths are read, ported or exercised:

| Upstream path                  | Use                                                    |
|--------------------------------|--------------------------------------------------------|
| `source/SMLib/`                | Kernel being ported (geometry, topology, algorithms)   |
| `source/SM_API/`               | High-level API being ported; reference for public ops  |
| `tests/SM_API_test/`           | Reference tests; source of difftest cases              |
| `tests/prog_test/`             | Reference tests; source of difftest cases              |
| `TestFiles/pt_TestFiles/`      | Test fixtures                                          |
| `AGENTS.md`, `.agents/`        | Upstream developer documentation (OCCT/USD files excluded) |
| `LICENSE`, `THIRD_PARTY_NOTICES.md` | Licence text and notices                          |

The C++ reference build (`harness/cpp`) compiles only `source/SMLib` and
`source/SM_API`, from a local clone that is never committed (`upstream/` is
gitignored).

## Upstream material not used

The following upstream layers are **not** read, ported, built or linked:

- OCCT translation: `source/OCCT_BREP_IMPORT`, `source/OCCT_BREP_EXPORT`,
  `TestFiles/occt_breps*`
- USD / Hydra integration: `source/SM_API_USD`, `source/BREP_SM_USD`,
  `source/BREP_USD_DATA`, `source/HYDRA_USD_BREP`, `source/USD_BREP_UTILS`,
  `source/schema`, `TestFiles/usd_TestFiles`
- Python bindings and tooling: `source/SmPyLib`, `source/SmPyDevLib`, `tools/`
- Proposals: `proposals/`

No GPL/LGPL code is used anywhere in solidcore.

## Per-file provenance

Every ported Rust file begins with a header naming the upstream source file(s) it
derives from and a table mapping its Rust items to upstream names (see
[`AGENTS.md`](https://github.com/tangency-labs/solidcore/blob/main/AGENTS.md#file-header)).

## Test corpora

These are fetched at run time (never committed) and used only by difftest's
`--ssi-corpus` correctness report (`crates/difftest/README.md`).

### ssi-corpus

- Repository: <https://github.com/jyviko/ssi-corpus> (fetched by
  `scripts/fetch-ssi-corpus.sh` into the gitignored `corpus/ssi/`)
- Pinned commit: `b04c8a6`
- Author: Iraklis Kourtis (jyviko), published for solidcore in
  [issue #17](https://github.com/tangency-labs/solidcore/issues/17)
- Licence: MIT OR Apache-2.0
- Copyright: (c) 2026 SolidCraft contributors
- Use: 119 surface/surface intersection cases with exact ground truth. The scorer in
  `crates/difftest/src/ssi/` (`score.rs`, `polyset.rs`, `geom.rs`) is ported from the
  corpus' `scoring/score.py` and `scoring/ssi_geom.py`, and the C++ shim
  `harness/cpp/shim/ssi_corpus.cpp` mirrors its `runner/ssi_runner.cpp`; each of those
  files says so in its header. The truth generators are not ported.

### Yang, Jia & Yan (2023)

- Yang, Jia and Yan, "Topology Guaranteed B-Spline Surface/Surface Intersection",
  *ACM Transactions on Graphics* 42(6), Article 211, 2023. Licence: CC-BY.
- Use: the corpus' 25 `y23_*` cases come from this paper (its published data, or surface
  pairs reconstructed from its figures); each such case cites it in its `source` field.

## Progress site

### d3-force

- Packages: `d3-force` 3.0.0 with its dependencies `d3-dispatch` 3.0.1, `d3-quadtree` 3.0.1
  and `d3-timer` 3.0.1, from npm (<https://github.com/d3/d3-force>)
- Licence: ISC (see [`site/assets/vendor/LICENSE-d3.txt`](site/assets/vendor/LICENSE-d3.txt))
- Copyright: (c) 2010-2021 Mike Bostock
- Use: the unmodified `dist/*.min.js` files, concatenated into
  `site/assets/vendor/d3-force.bundle.min.js`, lay out the dependency graph on the
  progress site (`graph.html`). They are served with the site and not loaded from a CDN.
