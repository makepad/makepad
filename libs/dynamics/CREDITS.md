# Credits

This crate is a Rust translation of the physics engine of MuJoCo 4.0.0
(https://github.com/google-deepmind/mujoco, tag `4.0.0`, commit
`ac60eae30fa7e0d31315c38d9476752fb81be67e`): its `src/engine` sources and the
`include/mujoco` headers they define, together with the SDF example plugin
`plugin/sdf/torus.cc` used as a collision test case.

    Copyright 2021 DeepMind Technologies Limited
    (later files: Copyright 2022-2026 DeepMind Technologies Limited)

    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        http://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.

The full licence text is in `LICENSE` beside this file. The source carries no
NOTICE file.

Changes: the C sources were translated to Rust, one module per source file
(the file each module comes from is named in its header). The rigid-body part
of the model compiler (`src/user/user_model.cc`, `user_objects.cc`,
`user_mesh.cc`, `user_util.cc`, `user_init.c` and the setters of `user_api.cc`)
was translated into the model builder (`builder.rs`, `model_spec.rs`) for the
subset this crate builds. The renderer, the visualisation, the user interface,
the XML parser, the rest of the model compiler, the Python bindings, the thread
pool and the GPU back ends were not translated. The MJCF writer
(`xml_export.rs`) is this crate's own; its attribute spellings and enum
mappings follow `src/xml/xml_native_reader.cc`. The convex hull (`hull.rs`) is
this crate's own. Fixed-size arenas became growable vectors.
