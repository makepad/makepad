# Rapid High Performance Rust Applications with Makepad and AI

## Talk Goal

Show that AI can now be used as an active application builder, not just a code autocomplete tool. With Makepad Studio, the AI can generate Rust UI code, run the app, inspect the visual result, click controls, type into fields, take screenshots, and iterate until the application behaves correctly.

The audience should leave with three ideas:

1. Rust is a strong target language for AI-generated applications because the compiler catches many mistakes early.
2. Makepad gives AI a fast feedback loop for real visual software, not only text-based programs.
3. High performance native, web, mobile, and XR applications can be created much faster when the IDE, runtime, compiler, and AI are connected.

## Opening

I am going to show how to rapidly create high performance Rust applications with Makepad.

The interesting part is not only that AI can write Rust. The interesting part is that the AI can work inside the Makepad IDE, run the application, inspect what it built, click buttons, enter text, take screenshots, and repair its own mistakes.

That changes the development loop. Instead of asking an AI for code and then manually testing everything, we can give the AI access to the same visual feedback loop that a human developer uses.

## Core Claim

Rust is an ideal language for AI application generation.

Not because AI writes perfect Rust on the first try. It does not. Rust is ideal because the compiler is strict, the type system is explicit, memory safety is checked, and performance is close to the metal. The compiler becomes a very strong correction mechanism for generated code.

Makepad adds the other missing piece: fast visual iteration. The AI does not only compile code. It can run the UI, inspect the widget tree, interact with the app, and improve it.

## Concept 1: AI as a Visual Developer

Traditional AI coding loop:

1. Ask for code.
2. Paste code into editor.
3. Run it yourself.
4. Describe the error or screenshot back to the AI.
5. Repeat.

Makepad Studio loop:

1. AI edits the application.
2. AI launches it through Studio.
3. AI reads compiler errors or runtime state.
4. AI inspects screenshots and widget trees.
5. AI clicks, types, and interacts with the app.
6. AI fixes the code and repeats.

The important shift is that the AI can observe the running graphical application directly. This makes it possible to iterate on layout, controls, rendering, input, and state without a human manually relaying every detail.

## Concept 2: Why Rust Works Well Here

Rust is useful for AI generation because it gives very concrete feedback:

- Wrong types are rejected.
- Ownership mistakes are caught.
- Missing fields and missing imports are explicit.
- Many runtime crashes become compile-time errors.
- Performance does not require switching to another language later.

For AI, a strict compiler is not friction. It is a steering system.

The AI can generate code, compile, read precise errors, patch the code, and repeat. The result is much more reliable than generating into a dynamic environment where many problems only appear while users are interacting with the application.

## Concept 3: Why Makepad Changes the Feedback Loop

Makepad is a Rust application framework and IDE for building high performance applications with custom UI and rendering.

It is designed around immediate visual feedback and cross-platform deployment:

- Desktop: Windows, macOS, Linux
- Web
- Mobile: Android and iOS
- XR: Quest

For this talk, the key Makepad capability is Studio automation. An AI assistant can run inside the Makepad development workflow and control the application through the Studio remote protocol.

That means the AI can validate visual software in the same loop where it writes the code.

## Concept 4: Performance Still Matters

AI-generated software should not mean slow software.

Makepad is built for high performance rendering and native deployment. The goal is not to generate disposable prototypes that must be rewritten later. The goal is to generate real Rust applications that can grow into production software.

That matters especially for the demos in this talk:

- CAD-style interaction
- 3D rendering
- Map rendering
- Vector rendering engines
- Mixed reality world scanning

These are not simple form demos. They require performance, responsiveness, and control over rendering.

## Demo Setup

Before the demos, explain the automation loop briefly:

1. I give the AI a goal.
2. The AI edits the Rust and Makepad UI code.
3. The AI runs the app from Makepad Studio.
4. The AI inspects the result.
5. The AI interacts with the application.
6. The AI fixes what is wrong.

The demo is not only the final application. The demo is the loop.

## Demo 1: Simple Application Generation

Goal: show the basic loop with something small and understandable.

Suggested demo:

- Ask the AI to create a compact productivity-style UI.
- Include a text input, a list, buttons, and state changes.
- Have the AI run it.
- Have the AI click into the input, type text, press return, and verify that the UI updated.

Talking points:

- This proves the AI is not only generating static code.
- It can interact with the result.
- The compiler catches structural problems.
- The Studio automation catches visual and behavioral problems.

What to emphasize:

The AI is now using the application like a user. That is the difference between code generation and automated application generation.

## Demo 2: AI-Generated CAD

Goal: show that the workflow can handle spatial tools and direct manipulation.

Suggested demo:

- Generate a small CAD-like editor.
- Add a canvas.
- Add tools for points, lines, rectangles, or simple shapes.
- Add selection and transform controls.
- Let the AI test by clicking points on the canvas.

Talking points:

- CAD needs precision and interactive feedback.
- The AI must reason about coordinates, hit testing, drawing, and state.
- This shows the system moving beyond forms and dashboards.

What to emphasize:

The hard part is not drawing one shape. The hard part is closing the loop between input, rendering, selection, and state. That is exactly where Studio automation helps.

## Demo 3: AI-Generated 3D

Goal: show high performance rendering and richer visual output.

Suggested demo:

- Generate a 3D scene.
- Add camera movement or object rotation.
- Add lighting, materials, and multiple objects.
- Have the AI verify that the scene is visible and interactive.

Talking points:

- 3D makes visual verification important.
- A compile-only check is not enough.
- The AI needs to know whether the object is actually on screen, framed correctly, and responding to input.

What to emphasize:

For visual applications, correctness includes what the user sees. Makepad Studio lets the AI inspect that.

## Demo 4: Map Rendering

Goal: show data-heavy rendering and real navigation patterns.

Suggested demo:

- Load or render map-like tiles or vector map data.
- Add pan and zoom.
- Add labels or markers.
- Ask the AI to adjust density, colors, or interaction behavior.

Talking points:

- Maps combine large datasets, rendering performance, and UI interaction.
- The AI can generate the UI controls and rendering logic, then inspect the result visually.
- Makepad gives enough control to keep the experience fast.

What to emphasize:

This is the kind of application where a web-only prototype often hits performance limits. Rust and Makepad allow the generated result to stay close to production constraints.

## Demo 5: Vector Engine

Goal: show custom rendering and visual design iteration.

Suggested demo:

- Generate a small vector drawing or icon engine.
- Add paths, fills, strokes, gradients, or animation.
- Let the AI create several visual variants.
- Have the AI inspect the output and refine spacing, color, and motion.

Talking points:

- Vector rendering is sensitive to detail.
- Small rendering mistakes are obvious visually.
- The AI can iterate against screenshots instead of guessing.

What to emphasize:

AI is strong at producing variations. Makepad gives it a runtime where those variations can be rendered and judged immediately.

## Demo 6: Mixed Reality World Scanning

Goal: show the upper end of the ambition: XR and spatial applications.

Suggested demo:

- Show a Quest or XR-oriented Makepad application.
- Demonstrate world scanning or a 3D spatial view.
- Add generated UI for inspecting captured geometry or spatial anchors.
- Discuss how the same Rust and Makepad approach can target XR.

Talking points:

- XR applications need performance, low latency, and direct control over rendering.
- The application is no longer a flat page.
- AI generation becomes more valuable because spatial UI has many moving parts.

What to emphasize:

The same automation idea applies: generate, run, inspect, interact, and fix. The target can be desktop, web, mobile, or XR.

## Transition Lines

Use these between demos if needed:

- "Now that we have seen the AI control a normal UI, let us make the interaction more spatial."
- "Compile success is only the first checkpoint. For visual software, the next question is: does it look and behave correctly?"
- "The point of this demo is not that the AI got everything right immediately. The point is that it can stay in the loop long enough to correct itself."
- "Rust gives us the hard boundary conditions. Makepad gives us the visual feedback loop."
- "This is where AI code generation starts looking less like autocomplete and more like an automated developer."

## Risks and Honest Framing

Be clear about what still matters:

- The AI still needs good goals.
- The AI still makes mistakes.
- Human taste and product judgment still matter.
- Complex architecture still benefits from human direction.
- Automated visual inspection is powerful, but it is not a replacement for all testing.

The strongest framing is not "AI replaces developers." The stronger framing is:

AI can now participate in the full application development loop, including visual runtime feedback. That makes a single developer much more capable, especially when paired with a strict language and a fast native framework.

## Closing

The old workflow was: write code by hand, compile, run, inspect, fix.

The new workflow is: describe the target, let the AI generate, let the compiler constrain it, let Makepad Studio show it the result, and let it iterate.

Rust gives us correctness and performance. Makepad gives us a high performance cross-platform runtime and a visual IDE that AI can control. Together, they make it possible to generate serious applications quickly: desktop tools, web apps, mobile apps, CAD systems, 3D scenes, map renderers, vector engines, and XR experiences.

The main message of this talk is simple:

We no longer need AI to only write snippets. We can let it build, run, see, click, and improve real high performance Rust applications.

## Short Version

If time is limited, use this sequence:

1. Explain Rust as the correction mechanism for AI-generated code.
2. Explain Makepad Studio as the visual feedback loop.
3. Demo a simple app that the AI generates and interacts with.
4. Demo one advanced visual example: CAD, 3D, maps, vectors, or XR.
5. Close on the idea that AI can now participate in the full application loop.

## One-Minute Pitch

In this talk I will show how to rapidly create high performance Rust applications with Makepad. You can use your AI of choice, including Codex or Claude, running inside the Makepad IDE.

Rust is an ideal language for AI generation because of its strong compiler guarantees and superb performance. We no longer need to write every line by hand. The compiler gives the AI precise feedback, and Makepad Studio gives it visual feedback.

Makepad Studio has automation integration for AI, so the AI can generate code, run the application, see the result, click buttons, type into fields, and iterate completely automatically on visual applications.

Makepad compiles to web, desktop, mobile, and XR. I will show how far we can push AI application generation, including CAD, 3D, map rendering, vector engines, and mixed reality world scanning.
