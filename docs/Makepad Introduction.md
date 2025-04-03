Makepad is an OSS cross-platform UI framework written in Rust, accompanied by Makepad Studio, a powerful authoring application. This combination offers a streamlined approach to UI development, enabling developers and designers to create modern, high-performance applications with ease.

Makepad is built with and for Rust, a modern, safe programming language. It operates natively and on the web, supporting all major platforms including Windows, Linux, macOS, iOS, and Android.

Makepad is known for its remarkably fast compilation speed which ensures a productive and interruption-free development experience.

Makepad UIs are shader based. This ensures high performance, making it suitable for building complex applications such as those similar to Photoshop and also for 2.5D UIs like one finds in VR/AR applications.

The framework simplifies styling compared to HTML by using an immediate mode and a simple DSL, eliminating the need for a complex DOM or CSS. It also provides direct access to the GPU, offering more control and flexibility while avoiding issues related to garbage collection, ensuring smoother performance.

One of Makepad's most stand out features is live styling, which allows immediate reflection of UI changes in the code and vice versa, without the need for recompilation or restarts. This feature significantly narrows the gap between developers and designers, enhancing overall productivity.
Makepad Studio brings it all together with its own IDE and Figma-like visual tooling, allowing designers to intuitively work directly on the actual product.

Another key component is Stitch, an experimental WebAssembly (WASM) interpreter built in Rust. Renowned for its exceptional speed and lightweight performance, Stitch is currently regarded as the fastest WASM interpreter available. [tbd: usage within Makepad. Superapps, extension system, allows plugging in building blocks without sacrificing performance …]