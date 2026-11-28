// Loaded synchronously as the first script in the app's head. Only the test server serves it.
(() => {
    // Rust reserves 0 for page errors, 1 for the GPU description and 2 for "rig loaded". Each step gets its own number from 3.
    // Keep errors in order with step results, including the final acknowledgement.
    let posted = Promise.resolve();
    function report(id, body) {
        posted = posted.then(async () => {
            const response = await fetch(`/.web-test/report/${id}`, {method: "POST", body});
            if (!response.ok) throw new Error(`report ${id}: HTTP ${response.status}`);
        }).catch(error => console.error("web test report failed", error));
    }
    function fail(error) {
        report(0, error?.stack || String(error));
    }
    window.addEventListener("error", event => fail(event.error || event.message ||
        `could not load ${event.target.src || event.target.href}`), true);
    window.addEventListener("unhandledrejection", event => fail(event.reason));

    report(2, "ready");

    let frame = 0;
    function tick() {
        frame++;
        requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
    const nextFrame = () => new Promise(resolve => requestAnimationFrame(resolve));

    let gl;
    let loss;
    let device;
    function identify() {
        const debug = gl.getExtension("WEBGL_debug_renderer_info");
        if (!debug) throw new Error("cannot identify GPU: WEBGL_debug_renderer_info unavailable");
        const renderer = gl.getParameter(debug.UNMASKED_RENDERER_WEBGL);
        const vendor = gl.getParameter(debug.UNMASKED_VENDOR_WEBGL);
        const info = `renderer=${renderer} vendor=${vendor}`;
        if (!renderer || /swiftshader|llvmpipe/i.test(info)) throw new Error(`real GPU required: ${info}`);
        return info;
    }
    const getContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function(type, ...args) {
        if (type !== "webgl2") return getContext.call(this, type, ...args);
        const context = getContext.call(this, type, {...args[0], preserveDrawingBuffer: true});
        if (!gl && context) {
            gl = context;
            // Let the app finish its own initialization on this context first.
            requestAnimationFrame(() => {
                try {
                    device = identify();
                    report(1, device);
                } catch (error) { fail(error); }
            });
        }
        return context;
    };

    // The app creates its context and publishes its stats once its wasm is running.
    async function until(ready, what) {
        const start = performance.now();
        while (!ready()) {
            if (performance.now() - start > 60000) throw new Error(`${what} did not appear within 60 s`);
            await nextFrame();
        }
    }

    async function settle() {
        await until(() => window.makepad_gpu_stats, "makepad_gpu_stats");
        const start = performance.now();
        let changed = start;
        let last = window.makepad_gpu_stats().frames;
        while (true) {
            await nextFrame();
            const now = performance.now();
            const frames = window.makepad_gpu_stats().frames;
            if (frames !== last) {
                last = frames;
                changed = now;
            }
            if (now - changed >= 500) return `quiet for 500 ms, GPU frames=${frames}`;
            if (now - start >= 10000) return `still drawing after 10 s, GPU frames=${frames}`;
        }
    }

    function grab() {
        if (gl.isContextLost()) throw new Error("cannot grab a lost context");
        const width = gl.drawingBufferWidth, height = gl.drawingBufferHeight;
        const bytes = new Uint8Array(width * height * 4);
        // Read the window even if the app left an offscreen FBO or a pack buffer bound.
        const fbo = gl.getParameter(gl.READ_FRAMEBUFFER_BINDING);
        const pbo = gl.getParameter(gl.PIXEL_PACK_BUFFER_BINDING);
        const pack = [gl.PACK_ALIGNMENT, gl.PACK_ROW_LENGTH, gl.PACK_SKIP_ROWS, gl.PACK_SKIP_PIXELS];
        const saved = pack.map(param => gl.getParameter(param));
        gl.bindFramebuffer(gl.READ_FRAMEBUFFER, null);
        const read = gl.getParameter(gl.READ_BUFFER);
        try {
            gl.readBuffer(gl.BACK);
            gl.bindBuffer(gl.PIXEL_PACK_BUFFER, null);
            pack.forEach((param, i) => gl.pixelStorei(param, i === 0 ? 1 : 0));
            gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, bytes);
            const error = gl.getError();
            if (error !== gl.NO_ERROR) throw new Error(`readPixels: GL error 0x${error.toString(16)}`);
        } finally {
            pack.forEach((param, i) => gl.pixelStorei(param, saved[i]));
            gl.bindBuffer(gl.PIXEL_PACK_BUFFER, pbo);
            gl.readBuffer(read);
            gl.bindFramebuffer(gl.READ_FRAMEBUFFER, fbo);
        }
        const header = new DataView(new ArrayBuffer(8));
        header.setUint32(0, width, true);
        header.setUint32(4, height, true);
        return new Blob([header, bytes]);
    }

    async function contextEvent(restore) {
        // A lost context cannot hand out extensions. Keep the handle used to lose it.
        if (gl.isContextLost() !== restore) throw new Error(restore ? "context is live" : "context is already lost");
        if (!restore) loss = gl.getExtension("WEBGL_lose_context");
        if (!loss) throw new Error("WEBGL_lose_context unavailable");
        const event = restore ? "webglcontextrestored" : "webglcontextlost";
        // This listener is added after the app's, so it sees whether the app cancelled the event.
        const arrived = new Promise(resolve => gl.canvas.addEventListener(event, resolve, {once: true}));
        if (restore) loss.restoreContext();
        else loss.loseContext();
        const timeout = new Promise(resolve => setTimeout(resolve, 10000, null));
        const got = await Promise.race([arrived, timeout]);
        if (!got) throw new Error(`no ${event} within 10 s${restore ? " (did the app cancel webglcontextlost?)" : ""}`);
        return restore ? event : `${event} defaultPrevented=${got.defaultPrevented}`;
    }

    window.makepadRigStep = async (id, command, count) => {
        try {
            let result;
            switch (command) {
                case "info": await until(() => device, "a WebGL2 context"); result = device; break;
                case "frames": {
                    const end = frame + count;
                    while (frame < end) await nextFrame();
                    result = `animation frame=${frame}`;
                    break;
                }
                case "settle": result = await settle(); break;
                case "grab": result = grab(); break;
                case "stats":
                    await until(() => window.makepad_gpu_stats && window.makepad_gpu_inventory, "makepad_gpu_stats and makepad_gpu_inventory");
                    result = JSON.stringify({stats: window.makepad_gpu_stats(), inventory: await window.makepad_gpu_inventory()}, null, 2);
                    break;
                case "lose": result = await contextEvent(false); break;
                case "restore": result = await contextEvent(true); break;
                // Include a renderer round trip after native input and before closing the page.
                case "ack": await nextFrame(); result = ""; break;
                default: throw new Error(`unknown rig command ${command}`);
            }
            report(id, result);
        } catch (error) { fail(error); }
    };
})();
