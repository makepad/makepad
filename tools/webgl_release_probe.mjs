// Usage: node tools/webgl_release_probe.mjs http://127.0.0.1:PORT/path [--map] [--text] [--lose-context] [--location] [--diagnostics]
// Bounded software-WebGL smoke test. It never uses a user profile, hardware GPU, or screenshots.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtempSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join} from 'node:path';

const PIXEL_BUDGET = 2_097_152;
const STARTUP_TIMEOUT_MS = 45_000;
const CDP_TIMEOUT_MS = 15_000;
const LOSS_WARNING_TIMEOUT_MS = 5_000;
const LOSS_QUIET_MS = 3_000;
const LOCATION_ACTION_TIMEOUT_MS = 5_000;
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
const RENDERER_REJECTION = /WebGL .*rejected|WebGL2 error|webgl\.compile_fail|webgl shaders:.*\bfailed\b|Missing shader|R32F render target.*unavailable|EXT_color_buffer_float unavailable/i;

const sleep = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

function parseArguments(argv) {
    const knownFlags = new Set(['--map', '--text', '--lose-context', '--location', '--diagnostics']);
    const flags = argv.filter(value => value.startsWith('--'));
    const urls = argv.filter(value => !value.startsWith('--'));
    for (const flag of flags) {
        assert(knownFlags.has(flag), `Unknown option: ${flag}`);
    }
    assert.equal(new Set(flags).size, flags.length, 'Options may only be supplied once');
    assert.equal(urls.length, 1, 'Supply exactly one explicit http://127.0.0.1:PORT URL');

    let parsed;
    try {
        parsed = new URL(urls[0]);
    } catch {
        assert.fail('Supply a valid explicit http://127.0.0.1:PORT URL');
    }
    assert.equal(parsed.protocol, 'http:', 'Only HTTP localhost test servers are allowed');
    assert.equal(parsed.hostname, '127.0.0.1', 'Only an explicit 127.0.0.1 URL is allowed');
    assert(parsed.port, 'The localhost URL must include an explicit port');
    assert.equal(parsed.username, '', 'URL credentials are not allowed');
    assert.equal(parsed.password, '', 'URL credentials are not allowed');
    return {
        url: parsed.href,
        map: flags.includes('--map'),
        text: flags.includes('--text'),
        loseContext: flags.includes('--lose-context'),
        location: flags.includes('--location'),
        diagnostics: flags.includes('--diagnostics'),
    };
}

function childHasExited(child, closed) {
    return closed.value || child.exitCode !== null || child.signalCode !== null;
}

async function waitForChildExit(child, closed, milliseconds) {
    if (childHasExited(child, closed)) return true;
    await Promise.race([
        new Promise(resolve => child.once('close', resolve)),
        sleep(milliseconds),
    ]);
    return childHasExited(child, closed);
}

function pageInstrumentation(diagnostics) {
    return String.raw`
(() => {
    const diagnostics = ${diagnostics ? 'true' : 'false'};
    const stats = window.__renderProbe = {
        draws: 0,
        mapDraws: 0,
        textDraws: 0,
        glyphInstances: 0,
        textShaderAttaches: 0,
        textProgramLinks: 0,
        textProgramUses: 0,
        drawByMethod: {},
        liveBuffers: 0,
        liveTextures: 0,
        bufferHighWater: 0,
        textureHighWater: 0,
        deletedBuffers: 0,
        deletedTextures: 0,
        maxBufferBytes: 0,
        maxTexturePixels: 0,
        invalidDraws: 0,
        contextLosses: 0,
        contextRestores: 0,
        contextLossAt: null,
        rafRequests: 0,
        rafCallbacks: 0,
        glErrors: [],
        uncaught: [],
    };
    const documentToken = Math.random().toString(36).slice(2) + '-' + Date.now();
    const contexts = [];
    const knownContexts = new WeakSet();
    const shaderIsMap = new WeakMap();
    const programIsMap = new WeakMap();
    const shaderTextSignatures = new WeakMap();
    const programShaders = new WeakMap();
    const linkedTextPrograms = new WeakSet();
    const shaderDiagnostics = new WeakMap();
    const textShaderInventory = [];
    const currentProgram = new WeakMap();
    const liveBuffers = new WeakSet();
    const liveTextures = new WeakSet();
    const patched = new WeakMap();

    const text = value => {
        try {
            if (value instanceof Error) return value.stack || value.message;
            return String(value);
        } catch (_) {
            return '[unprintable]';
        }
    };
    const keep = (array, value) => {
        if (array.length < 32) array.push(value);
    };
    const noteGlError = (gl, where, error) => {
        if (error !== gl.NO_ERROR) keep(stats.glErrors, {where, error});
    };
    const drainGlErrors = (gl, where) => {
        for (let i = 0; i < 16; i++) {
            const error = gl.getError();
            if (error === gl.NO_ERROR) return;
        }
    };
    const noteContext = gl => {
        if (!gl || knownContexts.has(gl)) return;
        knownContexts.add(gl);
        contexts.push(gl);
    };
    const samplerInventory = source => {
        const samplers = [];
        const pattern = /\b(?:uniform\s+)?(?:(?:lowp|mediump|highp)\s+)?sampler(?:2D|2DArray|3D|Cube)\s+([A-Za-z_]\w*)/g;
        for (const match of source.matchAll(pattern)) {
            if (!samplers.includes(match[1]) && samplers.length < 16) samplers.push(match[1]);
        }
        return samplers;
    };
    const classifyTextShader = source => {
        const signatures = [];
        const hasFontWord = /font|glyph|draw_text/i.test(source);
        const hasSlugWord = /slug/i.test(source);
        const hasCurve = /curve/i.test(source);
        const hasBand = /band/i.test(source);
        const hasRaster = /raster/i.test(source);
        const hasGlyph = /glyph/i.test(source);
        const hasTextShaderName = /DrawText|draw_text/i.test(source);
        const hasSlugAtlasFields = /font_t[12]/i.test(source)
            && /tex_coord[12]/i.test(source)
            && /curve|brightness/i.test(source);

        // DrawText's Slug path contains curve/band coverage logic; its older
        // atlas form exposes the paired font texture/coordinate fields.
        if ((hasCurve && hasBand && (hasSlugWord || hasFontWord)) || hasSlugAtlasFields) {
            signatures.push('slug-curve-band');
        }
        // Raster glyph shaders have explicit raster + glyph/font vocabulary.
        // Requiring both avoids treating an unrelated sampled image as text.
        if (hasRaster && (hasGlyph || hasFontWord)) signatures.push('raster-glyph');
        if (hasTextShaderName && (hasGlyph || hasRaster || samplerInventory(source).length > 0)) {
            signatures.push('draw-text');
        }
        return [...new Set(signatures)];
    };
    const patch = (object, name, hooks) => {
        if (!object || typeof object[name] !== 'function') return;
        let names = patched.get(object);
        if (!names) patched.set(object, names = new Set());
        if (names.has(name)) return;
        names.add(name);
        const original = object[name];
        object[name] = function(...args) {
            if (hooks.before) hooks.before.call(this, args);
            const result = original.apply(this, args);
            if (hooks.after) hooks.after.call(this, args, result);
            return result;
        };
    };
    const prototypes = [];
    if (typeof WebGLRenderingContext !== 'undefined') prototypes.push(WebGLRenderingContext.prototype);
    if (typeof WebGL2RenderingContext !== 'undefined') prototypes.push(WebGL2RenderingContext.prototype);

    const canvasGetContext = HTMLCanvasElement.prototype.getContext;
    HTMLCanvasElement.prototype.getContext = function(...args) {
        const result = canvasGetContext.apply(this, args);
        if (String(args[0]).toLowerCase() === 'webgl' || String(args[0]).toLowerCase() === 'experimental-webgl' || String(args[0]).toLowerCase() === 'webgl2') {
            noteContext(result);
        }
        return result;
    };

    const originalRaf = window.requestAnimationFrame.bind(window);
    window.requestAnimationFrame = callback => {
        stats.rafRequests++;
        return originalRaf(time => {
            stats.rafCallbacks++;
            return callback(time);
        });
    };

    window.addEventListener('error', event => {
        keep(stats.uncaught, text(event.error || event.message || 'window error'));
    }, true);
    window.addEventListener('unhandledrejection', event => {
        keep(stats.uncaught, text(event.reason || 'unhandled rejection'));
    }, true);
    document.addEventListener('webglcontextlost', () => {
        stats.contextLosses++;
        if (!stats.contextLossAt) {
            stats.contextLossAt = {
                draws: stats.draws,
                rafRequests: stats.rafRequests,
                rafCallbacks: stats.rafCallbacks,
            };
        }
    }, true);
    document.addEventListener('webglcontextrestored', () => stats.contextRestores++, true);

    for (const proto of prototypes) {
        patch(proto, 'shaderSource', {before(args) {
            const shader = args[0];
            const source = String(args[1] || '');
            shaderIsMap.set(shader, /shadow_mask|terrain_lift|tile_origin/.test(source));
            const signatures = classifyTextShader(source);
            shaderTextSignatures.set(shader, signatures);
            if (diagnostics && signatures.length > 0 && textShaderInventory.length < 16) {
                let type = 'unknown';
                try {
                    const shaderType = this.getShaderParameter(shader, this.SHADER_TYPE);
                    if (shaderType === this.VERTEX_SHADER) type = 'vertex';
                    if (shaderType === this.FRAGMENT_SHADER) type = 'fragment';
                } catch (_) {
                    // Shader type is diagnostic-only.
                }
                const entry = {
                    id: textShaderInventory.length + 1,
                    type,
                    signatures,
                    samplers: samplerInventory(source),
                    attaches: 0,
                    links: 0,
                };
                shaderDiagnostics.set(shader, entry);
                textShaderInventory.push(entry);
            }
        }});
        patch(proto, 'attachShader', {before(args) {
            programIsMap.set(args[0], Boolean(programIsMap.get(args[0]) || shaderIsMap.get(args[1])));
            let shaders = programShaders.get(args[0]);
            if (!shaders) programShaders.set(args[0], shaders = new Set());
            shaders.add(args[1]);
            const signatures = shaderTextSignatures.get(args[1]);
            if (signatures?.length) {
                stats.textShaderAttaches++;
                const entry = shaderDiagnostics.get(args[1]);
                if (entry) entry.attaches++;
            }
        }});
        patch(proto, 'detachShader', {before(args) {
            programShaders.get(args[0])?.delete(args[1]);
        }});
        patch(proto, 'linkProgram', {after(args) {
            const program = args[0];
            const signatures = new Set();
            const diagnosticEntries = new Set();
            for (const shader of programShaders.get(program) || []) {
                for (const signature of shaderTextSignatures.get(shader) || []) signatures.add(signature);
                const entry = shaderDiagnostics.get(shader);
                if (entry) diagnosticEntries.add(entry);
            }
            let linked = false;
            try {
                linked = true; // Actual draws plus GL-error checks prove usability without blocking asynchronous shader compilation.
            } catch (_) {
                // A failed link remains ineligible for text draw accounting.
            }
            if (linked && signatures?.size) {
                linkedTextPrograms.add(program);
                stats.textProgramLinks++;
                if (diagnostics) {
                    for (const entry of diagnosticEntries) entry.links++;
                }
            } else {
                linkedTextPrograms.delete(program);
            }
        }});
        patch(proto, 'useProgram', {after(args) {
            currentProgram.set(this, args[0]);
            if (linkedTextPrograms.has(args[0])) stats.textProgramUses++;
        }});
        patch(proto, 'createBuffer', {after(_args, result) {
            if (result && !liveBuffers.has(result)) {
                liveBuffers.add(result);
                stats.liveBuffers++;
                stats.bufferHighWater = Math.max(stats.bufferHighWater, stats.liveBuffers);
            }
        }});
        patch(proto, 'deleteBuffer', {after(args) {
            if (args[0] && liveBuffers.has(args[0])) {
                liveBuffers.delete(args[0]);
                stats.liveBuffers--;
                stats.deletedBuffers++;
            }
        }});
        patch(proto, 'createTexture', {after(_args, result) {
            if (result && !liveTextures.has(result)) {
                liveTextures.add(result);
                stats.liveTextures++;
                stats.textureHighWater = Math.max(stats.textureHighWater, stats.liveTextures);
            }
        }});
        patch(proto, 'deleteTexture', {after(args) {
            if (args[0] && liveTextures.has(args[0])) {
                liveTextures.delete(args[0]);
                stats.liveTextures--;
                stats.deletedTextures++;
            }
        }});
        patch(proto, 'bufferData', {before(args) {
            const data = args[1];
            let bytes = typeof data === 'number' ? data : Number(data && data.byteLength) || 0;
            if (ArrayBuffer.isView(data) && Number.isFinite(args[4])) {
                bytes = Math.max(0, args[4]) * (data.BYTES_PER_ELEMENT || 1);
            }
            stats.maxBufferBytes = Math.max(stats.maxBufferBytes, bytes);
        }});
        const textureSizeArguments = {
            texImage2D: [3, 4, 9],
            texSubImage2D: [4, 5, 9],
            texStorage2D: [3, 4, 5],
            texImage3D: [3, 4, 10],
            texSubImage3D: [5, 6, 11],
            texStorage3D: [3, 4, 6],
        };
        for (const [name, [widthIndex, heightIndex, explicitLength]] of Object.entries(textureSizeArguments)) {
            patch(proto, name, {before(args) {
                let width;
                let height;
                if (args.length >= explicitLength) {
                    width = Number(args[widthIndex]);
                    height = Number(args[heightIndex]);
                } else {
                    const source = args[args.length - 1];
                    width = Number(source && (source.videoWidth || source.naturalWidth || source.width));
                    height = Number(source && (source.videoHeight || source.naturalHeight || source.height));
                }
                if (Number.isFinite(width) && Number.isFinite(height) && width >= 0 && height >= 0) {
                    stats.maxTexturePixels = Math.max(stats.maxTexturePixels, width * height);
                }
            }});
        }
        patch(proto, 'getError', {after(_args, result) {
            noteGlError(this, 'getError', result);
        }});
    }

    const drawSpecs = {
        drawArrays: [1, 2],
        drawElements: [1, 3],
        drawRangeElements: [1, 2, 3, 5],
        drawArraysInstanced: [1, 2, 3],
        drawElementsInstanced: [1, 3, 4],
    };
    const positive = value => Number.isFinite(Number(value)) && Number(value) > 0;
    const instancedTextDraw = (name, args) => {
        if (name === 'drawArraysInstanced' || name === 'drawArraysInstancedANGLE') {
            return positive(args[2]) && positive(args[3]) ? {draws: 1, instances: Number(args[3])} : null;
        }
        if (name === 'drawElementsInstanced' || name === 'drawElementsInstancedANGLE') {
            return positive(args[1]) && positive(args[4]) ? {draws: 1, instances: Number(args[4])} : null;
        }
        const arrays = name === 'multiDrawArraysInstancedWEBGL';
        const elements = name === 'multiDrawElementsInstancedWEBGL';
        if (!arrays && !elements) return null;
        const counts = args[1];
        const countsOffset = Number(args[2]);
        const instanceCounts = args[arrays ? 5 : 6];
        const instanceCountsOffset = Number(args[arrays ? 6 : 7]);
        const drawCount = Number(args[arrays ? 7 : 8]);
        if (!counts || !instanceCounts || !Number.isInteger(countsOffset)
            || !Number.isInteger(instanceCountsOffset) || !Number.isInteger(drawCount) || drawCount <= 0) return null;
        let draws = 0;
        let instances = 0;
        for (let index = 0; index < drawCount; index++) {
            const count = counts[countsOffset + index];
            const instanceCount = instanceCounts[instanceCountsOffset + index];
            if (positive(count) && positive(instanceCount)) {
                draws++;
                instances += Number(instanceCount);
            }
        }
        return draws > 0 ? {draws, instances} : null;
    };
    const recordDraw = (gl, name, args, numericArguments) => {
        stats.draws++;
        stats.drawByMethod[name] = (stats.drawByMethod[name] || 0) + 1;
        const program = currentProgram.get(gl);
        if (programIsMap.get(program)) stats.mapDraws++;
        if (linkedTextPrograms.has(program)) {
            const textDraw = instancedTextDraw(name, args);
            if (textDraw) {
                stats.textDraws += textDraw.draws;
                stats.glyphInstances += textDraw.instances;
            }
        }
        if (numericArguments.some(index => !Number.isFinite(args[index]))) stats.invalidDraws++;
        drainGlErrors(gl, name);
    };
    for (const proto of prototypes) {
        for (const [name, numericArguments] of Object.entries(drawSpecs)) {
            patch(proto, name, {after(args) {
                recordDraw(this, name, args, numericArguments);
            }});
        }
        patch(proto, 'getExtension', {after(_args, extension) {
            if (!extension) return;
            const owningGl = this;
            for (const [name, numericArguments] of Object.entries({
                drawArraysInstancedANGLE: [1, 2, 3],
                drawElementsInstancedANGLE: [1, 3, 4],
                multiDrawArraysWEBGL: [],
                multiDrawElementsWEBGL: [],
                multiDrawArraysInstancedWEBGL: [],
                multiDrawElementsInstancedWEBGL: [],
            })) {
                patch(extension, name, {after(args) {
                    recordDraw(owningGl, name, args, numericArguments);
                }});
            }
        }});
    }

    const visibleText = () => document.body ? document.body.innerText : '';
    const rendererName = gl => {
        try {
            const extension = gl.getExtension('WEBGL_debug_renderer_info');
            if (extension) return String(gl.getParameter(extension.UNMASKED_RENDERER_WEBGL));
            return String(gl.getParameter(gl.RENDERER));
        } catch (error) {
            return '[renderer query failed: ' + text(error) + ']';
        }
    };
    window.__renderProbeSnapshot = () => {
        for (const gl of contexts) {
            if (!gl.isContextLost()) drainGlErrors(gl, 'snapshot');
        }
        return {
            documentToken,
            title: document.title,
            stats,
            ...(diagnostics ? {textShaderInventory} : {}),
            visibleReloads: (visibleText().match(/\breload\b/gi) || []).length,
            canvases: Array.from(document.querySelectorAll('canvas')).map(canvas => {
                const gl = contexts.find(context => context.canvas === canvas);
                return {
                    width: canvas.width,
                    height: canvas.height,
                    cssWidth: canvas.clientWidth,
                    cssHeight: canvas.clientHeight,
                    live: canvas.isConnected && canvas.width > 0 && canvas.height > 0 && canvas.clientWidth > 0 && canvas.clientHeight > 0,
                    hasWebgl: Boolean(gl),
                    contextLost: gl ? gl.isContextLost() : false,
                    renderer: gl ? rendererName(gl) : null,
                };
            }),
        };
    };
    window.__renderProbeLoseContext = () => {
        const candidates = contexts.filter(gl => gl.canvas && gl.canvas.isConnected && gl.canvas.width > 0 && gl.canvas.height > 0);
        candidates.sort((a, b) => b.canvas.width * b.canvas.height - a.canvas.width * a.canvas.height);
        const gl = candidates[0];
        if (!gl) return {ok: false, error: 'No live app WebGL canvas'};
        const extension = gl.getExtension('WEBGL_lose_context');
        if (!extension) return {ok: false, error: 'WEBGL_lose_context is unavailable'};
        extension.loseContext();
        return {ok: true};
    };
})();
`;
}

function locationInstrumentation() {
    return String.raw`
(() => {
    const watchers = new Map();
    const pendingGets = [];
    let nextWatchId = 1;
    const calls = {
        watchPosition: 0,
        getCurrentPosition: 0,
        clearWatch: 0,
        clearedIds: [],
    };
    const position = fix => ({
        coords: {
            longitude: fix.longitude,
            latitude: fix.latitude,
            accuracy: fix.accuracy,
            altitude: null,
            altitudeAccuracy: null,
            heading: null,
            speed: null,
        },
        timestamp: fix.timestamp,
    });
    const geolocation = {
        watchPosition(success, error, options) {
            const id = nextWatchId++;
            calls.watchPosition++;
            watchers.set(id, {success, error, options});
            return id;
        },
        getCurrentPosition(success, error, options) {
            calls.getCurrentPosition++;
            pendingGets.push({success, error, options});
        },
        clearWatch(id) {
            calls.clearWatch++;
            calls.clearedIds.push(id);
            watchers.delete(id);
        },
    };
    Object.defineProperty(navigator, 'geolocation', {
        configurable: true,
        enumerable: true,
        value: geolocation,
    });
    window.__locationProbe = {
        snapshot() {
            return {
                watchPosition: calls.watchPosition,
                getCurrentPosition: calls.getCurrentPosition,
                clearWatch: calls.clearWatch,
                clearedIds: calls.clearedIds.slice(),
                activeWatches: watchers.size,
                pendingGets: pendingGets.length,
            };
        },
        deny() {
            const error = {code: 1, message: 'Permission denied by location consent probe'};
            for (const watcher of Array.from(watchers.values())) {
                if (typeof watcher.error === 'function') watcher.error(error);
            }
            for (const request of pendingGets.splice(0)) {
                if (typeof request.error === 'function') request.error(error);
            }
        },
        fix(fix) {
            const value = position(fix);
            for (const watcher of Array.from(watchers.values())) watcher.success(value);
            for (const request of pendingGets.splice(0)) request.success(value);
        },
    };
})();
`;
}

function isLiveWebGlCanvas(canvas) {
    return canvas.live && canvas.hasWebgl;
}

function assertPixelBudget(snapshot, phase) {
    assert(snapshot.canvases.length > 0, `No canvas found during ${phase}`);
    assert(
        snapshot.canvases.every(canvas => canvas.width * canvas.height <= PIXEL_BUDGET),
        `Canvas exceeded ${PIXEL_BUDGET} pixels during ${phase}`,
    );
}

function assertNoRenderWorkAfterContextLoss(snapshot, phase) {
    const baseline = snapshot.stats.contextLossAt;
    assert(baseline, `Missing event-time WebGL context-loss counters during ${phase}`);
    assert.equal(snapshot.stats.draws, baseline.draws, `Renderer submitted a draw after WebGL context loss during ${phase}`);
    assert.equal(snapshot.stats.rafRequests, baseline.rafRequests, `Renderer requested an animation frame after WebGL context loss during ${phase}`);
    assert.equal(snapshot.stats.rafCallbacks, baseline.rafCallbacks, `Renderer ran requestAnimationFrame after WebGL context loss during ${phase}`);
}

function resultSummary(snapshot, mode, fonts, diagnostics, extra = {}) {
    const stats = snapshot.stats;
    return {
        status: 'PASS',
        mode,
        title: snapshot.title,
        draws: stats.draws,
        mapDraws: stats.mapDraws,
        textDraws: stats.textDraws,
        glyphInstances: stats.glyphInstances,
        drawByMethod: stats.drawByMethod,
        fonts,
        renderers: [...new Set(snapshot.canvases.filter(canvas => canvas.hasWebgl).map(canvas => canvas.renderer))],
        buffers: {
            live: stats.liveBuffers,
            highWater: stats.bufferHighWater,
            deleted: stats.deletedBuffers,
            maxBytes: stats.maxBufferBytes,
        },
        textures: {
            live: stats.liveTextures,
            highWater: stats.textureHighWater,
            deleted: stats.deletedTextures,
            maxPixels: stats.maxTexturePixels,
        },
        ...(diagnostics ? {
            diagnostics: {
                textShaderAttaches: stats.textShaderAttaches,
                textProgramLinks: stats.textProgramLinks,
                textProgramUses: stats.textProgramUses,
                textShaders: snapshot.textShaderInventory || [],
                consoleWarnings: diagnostics.consoleWarnings,
            },
        } : {}),
        ...extra,
    };
}

async function run() {
    const options = parseArguments(process.argv.slice(2));
    const profile = mkdtempSync(join(tmpdir(), 'makepad-webgl-release-probe-'));
    let child;
    let socket;
    let childStderr = '';
    let pageSessionId;
    let sequence = 0;
    let mainFrameNavigations = 0;
    const pending = new Map();
    const runtimeErrors = [];
    const rendererRejections = [];
    const fetchedErrorReports = [];
    const fontRequests = new Map();
    const fonts = {successes: [], failures: []};
    const consoleWarnings = [];
    const closed = {value: false};
    let locationCounts;

    const rejectPending = error => {
        for (const request of pending.values()) {
            clearTimeout(request.timeout);
            request.reject(error);
        }
        pending.clear();
    };
    const send = (method, params = {}, sessionId, timeoutMs = CDP_TIMEOUT_MS) => new Promise((resolve, reject) => {
        if (!socket || socket.readyState !== WebSocket.OPEN) {
            reject(new Error(`CDP socket is not open for ${method}`));
            return;
        }
        const id = ++sequence;
        const timeout = setTimeout(() => {
            pending.delete(id);
            reject(new Error(`CDP timeout: ${method}`));
        }, timeoutMs);
        pending.set(id, {resolve, reject, timeout});
        socket.send(JSON.stringify({id, method, params, ...(sessionId ? {sessionId} : {})}));
    });
    const command = (method, params = {}) => send(method, params, pageSessionId);
    const consoleText = args => args.map(value => value.value ?? value.unserializableValue ?? value.description ?? '').join(' ');
    const captureFetchedReport = request => {
        try {
            const requestUrl = new URL(request.url);
            let report;
            if (requestUrl.pathname === '/api/crash') {
                report = request.postData || '[crash report without captured body]';
            } else if (requestUrl.pathname === '/$report_error' && requestUrl.searchParams.has('data')) {
                report = requestUrl.searchParams.get('data');
            }
            if (report !== undefined && fetchedErrorReports.length < 32) {
                fetchedErrorReports.push(String(report).slice(0, 4000));
            }
        } catch {
            // A malformed request URL will be surfaced by the app if it matters.
        }
    };
    const isHttpUrl = value => /^https?:\/\//i.test(String(value || ''));
    const isFontResource = (type, url, mimeType = '') => type === 'Font'
        || /^font\//i.test(mimeType)
        || /\.(?:woff2?|ttf|otf)(?:$|[?#])/i.test(String(url || ''));
    const boundedFontUrl = value => String(value || '').slice(0, 1_000);
    const keepUniqueFont = (array, value) => {
        if (array.length >= 32) return;
        if (!array.some(existing => JSON.stringify(existing) === JSON.stringify(value))) array.push(value);
    };
    const captureFontResponse = params => {
        const response = params.response || {};
        if (!isHttpUrl(response.url) || !isFontResource(params.type, response.url, response.mimeType)) return;
        fontRequests.set(params.requestId, response.url);
        const value = {
            url: boundedFontUrl(response.url),
            status: response.status,
            mimeType: String(response.mimeType || '').slice(0, 200),
        };
        if (response.status >= 200 && response.status < 400) keepUniqueFont(fonts.successes, value);
        else keepUniqueFont(fonts.failures, value);
    };
    const captureFontFailure = params => {
        const url = fontRequests.get(params.requestId);
        if (!isHttpUrl(url) && params.type !== 'Font') return;
        keepUniqueFont(fonts.failures, {
            url: boundedFontUrl(url || '[font URL unavailable]'),
            error: String(params.errorText || 'loading failed').slice(0, 500),
            canceled: Boolean(params.canceled),
        });
    };

    const evaluate = async expression => {
        const response = await command('Runtime.evaluate', {expression, returnByValue: true, awaitPromise: true});
        if (response.exceptionDetails) {
            throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
        }
        return response.result.value;
    };
    const snapshot = async () => {
        const value = await evaluate('window.__renderProbeSnapshot ? window.__renderProbeSnapshot() : null');
        assert(value, 'Probe instrumentation is unavailable in the app document');
        return value;
    };
    const locationSnapshot = async () => {
        const value = await evaluate('window.__locationProbe ? window.__locationProbe.snapshot() : null');
        assert(value, 'Location probe instrumentation is unavailable in the app document');
        return value;
    };
    const assertLocationCounts = (current, phase, watches, clears, activeWatches) => {
        assert.deepEqual({
            watches: current.watchPosition,
            currentRequests: current.getCurrentPosition,
            clears: current.clearWatch,
            activeWatches: current.activeWatches,
            pendingCurrentRequests: current.pendingGets,
        }, {
            watches,
            currentRequests: 0,
            clears,
            activeWatches,
            pendingCurrentRequests: 0,
        }, `Unexpected geolocation calls during ${phase}`);
    };
    const waitForLocation = async (predicate, phase) => {
        const deadline = Date.now() + LOCATION_ACTION_TIMEOUT_MS;
        let current;
        while (Date.now() < deadline) {
            current = await locationSnapshot();
            if (predicate(current)) return current;
            await sleep(100);
        }
        assert.fail(`Timed out waiting for geolocation state during ${phase}: ${JSON.stringify(current)}`);
    };
    const expectLocation = async (phase, watches, clears, activeWatches, {wait = false, settle = 0} = {}) => {
        if (settle) await sleep(settle);
        const current = wait
            ? await waitForLocation(value => value.watchPosition >= watches && value.clearWatch >= clears, phase)
            : await locationSnapshot();
        assertLocationCounts(current, phase, watches, clears, activeWatches);
        return current;
    };
    const clickLocationControl = async () => {
        const point = await evaluate(`(() => {
            const canvases = Array.from(document.querySelectorAll('canvas'))
                .filter(canvas => canvas.isConnected && canvas.clientWidth > 0 && canvas.clientHeight > 0)
                .sort((a, b) => b.clientWidth * b.clientHeight - a.clientWidth * a.clientHeight);
            const canvas = canvases[0];
            if (!canvas) return null;
            const localX = 165;
            const localY = canvas.clientHeight - 36;
            if (localX < 0 || localX >= canvas.clientWidth || localY < 0 || localY >= canvas.clientHeight) return null;
            const rect = canvas.getBoundingClientRect();
            return {
                x: rect.left + localX * rect.width / canvas.clientWidth,
                y: rect.top + localY * rect.height / canvas.clientHeight,
            };
        })()`);
        assert(point, 'Known location-control coordinate is outside the live canvas');
        await command('Input.dispatchMouseEvent', {type: 'mouseMoved', x: point.x, y: point.y, button: 'none', buttons: 0});
        await command('Input.dispatchMouseEvent', {type: 'mousePressed', x: point.x, y: point.y, button: 'left', buttons: 1, clickCount: 1});
        await command('Input.dispatchMouseEvent', {type: 'mouseReleased', x: point.x, y: point.y, button: 'left', buttons: 0, clickCount: 1});
    };
    const allErrors = current => [
        ...runtimeErrors,
        ...rendererRejections,
        ...fetchedErrorReports.map(report => `fetch error report: ${report}`),
        ...(current?.stats?.uncaught || []),
    ];
    const assertClean = (current, expectedContextLosses = 0) => {
        const errors = allErrors(current);
        assert.equal(errors.length, 0, errors.join('\n').slice(0, 12_000));
        assert.equal(current.stats.invalidDraws, 0, 'Invalid WebGL draw arguments observed');
        assert.deepEqual(current.stats.glErrors, [], 'WebGL errors observed');
        assert.equal(current.stats.contextLosses, expectedContextLosses, 'Unexpected WebGL context loss count');
        assert.equal(current.stats.contextRestores, 0, 'Unexpected WebGL context restoration');
    };

    try {
        child = spawn(CHROME, [
            '--headless=new',
            '--use-gl=angle',
            '--use-angle=swiftshader',
            '--enable-unsafe-swiftshader',
            '--disable-background-networking',
            '--disable-component-update',
            '--disable-sync',
            '--disable-extensions',
            '--disable-default-apps',
            '--no-first-run',
            '--no-default-browser-check',
            '--remote-debugging-port=0',
            `--user-data-dir=${profile}`,
            'about:blank',
        ], {stdio: ['ignore', 'ignore', 'pipe']});
        child.once('close', () => { closed.value = true; });
        child.stderr.on('data', data => {
            childStderr = (childStderr + data).slice(-12_000);
        });

        const endpoint = await new Promise((resolve, reject) => {
            const timeout = setTimeout(() => reject(new Error(`Chrome launch timeout\n${childStderr}`)), 20_000);
            const finish = callback => value => {
                clearTimeout(timeout);
                callback(value);
            };
            child.once('error', finish(reject));
            child.once('exit', finish(code => reject(new Error(`Chrome exited before CDP was ready: ${code}\n${childStderr}`))));
            child.stderr.on('data', () => {
                const match = childStderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
                if (match) finish(resolve)(match[1]);
            });
        });

        socket = new WebSocket(endpoint);
        await new Promise((resolve, reject) => {
            const timeout = setTimeout(() => reject(new Error('CDP WebSocket connection timeout')), 10_000);
            socket.onopen = () => { clearTimeout(timeout); resolve(); };
            socket.onerror = () => { clearTimeout(timeout); reject(new Error('CDP WebSocket connection failed')); };
        });
        socket.onclose = () => rejectPending(new Error('CDP WebSocket closed'));
        socket.onmessage = event => {
            let message;
            try {
                message = JSON.parse(event.data);
            } catch (error) {
                runtimeErrors.push(`Invalid CDP message: ${error}`);
                return;
            }
            if (message.id) {
                const request = pending.get(message.id);
                if (!request) return;
                pending.delete(message.id);
                clearTimeout(request.timeout);
                if (message.error) request.reject(new Error(JSON.stringify(message.error)));
                else request.resolve(message.result);
                return;
            }
            if (message.sessionId !== pageSessionId) return;
            if (message.method === 'Runtime.exceptionThrown') {
                runtimeErrors.push(message.params.exceptionDetails.exception?.description || message.params.exceptionDetails.text);
            } else if (message.method === 'Runtime.consoleAPICalled') {
                const value = consoleText(message.params.args);
                if (RENDERER_REJECTION.test(value)) rendererRejections.push(value);
                if (options.diagnostics && /^(?:warning|error)$/.test(message.params.type) && consoleWarnings.length < 16) {
                    consoleWarnings.push(value.slice(0, 1_000));
                }
            } else if (message.method === 'Log.entryAdded') {
                const value = message.params.entry.text || '';
                if (RENDERER_REJECTION.test(value)) rendererRejections.push(value);
                if (options.diagnostics && /^(?:warning|error)$/.test(message.params.entry.level) && consoleWarnings.length < 16) {
                    consoleWarnings.push(value.slice(0, 1_000));
                }
            } else if (message.method === 'Network.requestWillBeSent') {
                captureFetchedReport(message.params.request);
                if (isHttpUrl(message.params.request.url)
                    && isFontResource(message.params.type, message.params.request.url)) {
                    fontRequests.set(message.params.requestId, message.params.request.url);
                }
            } else if (message.method === 'Network.responseReceived') {
                captureFontResponse(message.params);
            } else if (message.method === 'Network.loadingFailed') {
                captureFontFailure(message.params);
            } else if (message.method === 'Page.frameNavigated' && !message.params.frame.parentId) {
                mainFrameNavigations++;
            }
        };

        const targets = await send('Target.getTargets');
        const target = targets.targetInfos.find(info => info.type === 'page' && info.url === 'about:blank')
            || targets.targetInfos.find(info => info.type === 'page');
        assert(target, 'Chrome did not expose an owned page target');
        ({sessionId: pageSessionId} = await send('Target.attachToTarget', {targetId: target.targetId, flatten: true}));
        await Promise.all([
            command('Runtime.enable'),
            command('Page.enable'),
            command('Network.enable'),
            command('Log.enable'),
        ]);
        await command('Page.addScriptToEvaluateOnNewDocument', {source: pageInstrumentation(options.diagnostics)});
        if (options.location) {
            await command('Page.addScriptToEvaluateOnNewDocument', {source: locationInstrumentation()});
        }
        await command('Emulation.setDeviceMetricsOverride', {width: 900, height: 640, deviceScaleFactor: 3, mobile: false});
        await command('Page.navigate', {url: options.url});

        const startupDeadline = Date.now() + STARTUP_TIMEOUT_MS;
        let initial;
        while (Date.now() < startupDeadline) {
            await sleep(500);
            try {
                initial = await snapshot();
            } catch (error) {
                if (Date.now() >= startupDeadline) throw error;
                continue;
            }
            // Cached idle maps legitimately render nine setup draws and then stop.
            const drewEnough = initial.stats.draws > 0
                && (!options.map || initial.stats.mapDraws > 0)
                && (!options.text || (initial.stats.textDraws > 0 && initial.stats.glyphInstances > 0));
            if (drewEnough && initial.canvases.some(isLiveWebGlCanvas)) break;
            if (allErrors(initial).length > 0) break;
        }
        if (options.diagnostics && initial
            && (allErrors(initial).length > 0
                || !initial.canvases.some(isLiveWebGlCanvas)
                || (options.text && !(initial.stats.textDraws > 0 && initial.stats.glyphInstances > 0)))) {
            console.error(JSON.stringify({
                status: 'DIAGNOSTICS',
                textDraws: initial.stats.textDraws,
                glyphInstances: initial.stats.glyphInstances,
                textShaderAttaches: initial.stats.textShaderAttaches,
                textProgramLinks: initial.stats.textProgramLinks,
                textProgramUses: initial.stats.textProgramUses,
                textShaders: initial.textShaderInventory || [],
                consoleWarnings,
                fonts,
            }));
        }
        assert(initial, 'No instrumented app document became available within 45 seconds');
        assertClean(initial);
        assert(initial.canvases.some(isLiveWebGlCanvas), 'No live WebGL canvas became available within 45 seconds');
        if (options.map) {
            assert(initial.stats.mapDraws > 0, 'No map shader draws observed within 45 seconds');
        }
        if (options.text) {
            assert(initial.stats.textDraws > 0, 'No Makepad text shader draws observed within 45 seconds');
            assert(initial.stats.glyphInstances > 0, 'No positive-count glyph instances observed within 45 seconds');
        }
        assert(initial.stats.draws > 0, 'No WebGL draw calls observed within 45 seconds');
        assertPixelBudget(initial, 'retina startup');
        if (options.location) {
            locationCounts = await expectLocation('startup', 0, 0, 0);
        }

        await command('Emulation.setDeviceMetricsOverride', {width: 1700, height: 1000, deviceScaleFactor: 3, mobile: false});
        await sleep(2_500);
        const resized = await snapshot();
        assertClean(resized);
        assert(resized.canvases.some(isLiveWebGlCanvas), 'WebGL canvas was no longer live after resize');
        assertPixelBudget(resized, 'large retina resize');
        if (options.location) {
            locationCounts = await expectLocation('resize before consent', 0, 0, 0);

            await clickLocationControl();
            locationCounts = await expectLocation('first explicit click', 1, 0, 1, {wait: true});

            await clickLocationControl();
            locationCounts = await expectLocation('pending second click', 1, 0, 1, {settle: 500});

            await evaluate('window.__locationProbe.deny()');
            locationCounts = await expectLocation('permission denial cleanup', 1, 1, 0, {wait: true});
            assert.deepEqual(locationCounts.clearedIds, [1], 'Permission denial cleared an unexpected watch');

            await clickLocationControl();
            locationCounts = await expectLocation('explicit retry', 2, 1, 1, {wait: true});

            await evaluate(`window.__locationProbe.fix({
                longitude: 4.8952,
                latitude: 52.3702,
                accuracy: 25,
                timestamp: Date.now(),
            })`);
            locationCounts = await expectLocation('synthetic fix', 2, 1, 1, {settle: 500});

            await clickLocationControl();
            locationCounts = await expectLocation('explicit recenter', 2, 1, 1, {settle: 500});
            assertClean(await snapshot());
        }
        const baseMode = `${options.map ? 'map' : 'generic'}${options.text ? '+text' : ''}`;
        const diagnostics = options.diagnostics ? {consoleWarnings} : null;
        console.log(JSON.stringify(resultSummary(resized, baseMode, fonts, diagnostics)));

        if (options.loseContext) {
            const errorsBeforeLoss = {
                runtime: runtimeErrors.length,
                renderer: rendererRejections.length,
                fetched: fetchedErrorReports.length,
                uncaught: resized.stats.uncaught.length,
            };
            const navigationAtLoss = mainFrameNavigations;
            const tokenAtLoss = resized.documentToken;
            const loss = await evaluate('window.__renderProbeLoseContext()');
            assert(loss?.ok, loss?.error || 'Failed to request WebGL context loss');

            const warningDeadline = Date.now() + LOSS_WARNING_TIMEOUT_MS;
            let terminal;
            while (Date.now() < warningDeadline) {
                await sleep(100);
                try {
                    terminal = await snapshot();
                } catch {
                    continue;
                }
                if (terminal.stats.contextLosses === 1 && terminal.visibleReloads === 1) break;
            }
            assert(terminal, 'App document disappeared after WebGL context loss');
            assert.equal(terminal.documentToken, tokenAtLoss, 'App automatically reloaded after WebGL context loss');
            assert.equal(mainFrameNavigations, navigationAtLoss, 'App navigated after WebGL context loss');
            assert.equal(terminal.stats.contextLosses, 1, 'Expected exactly one WebGL context loss event');
            assert.equal(terminal.stats.contextRestores, 0, 'WebGL context restored after terminal loss');
            assert.equal(terminal.visibleReloads, 1, 'Expected exactly one visible terminal Reload warning');
            assertNoRenderWorkAfterContextLoss(terminal, 'terminal warning');

            await sleep(LOSS_QUIET_MS);
            const quiet = await snapshot();
            assert.equal(quiet.documentToken, tokenAtLoss, 'App automatically reloaded during the context-loss quiet interval');
            assert.equal(mainFrameNavigations, navigationAtLoss, 'App navigated during the context-loss quiet interval');
            assert.equal(quiet.stats.contextLosses, 1, 'Context-loss event repeated during the quiet interval');
            assert.equal(quiet.stats.contextRestores, 0, 'WebGL context restored during the quiet interval');
            assert.equal(quiet.visibleReloads, 1, 'Terminal Reload warning changed during the quiet interval');
            assertNoRenderWorkAfterContextLoss(quiet, 'quiet interval');
            assert.equal(runtimeErrors.length, errorsBeforeLoss.runtime, 'Uncaught exception followed intentional context loss');
            assert.equal(rendererRejections.length, errorsBeforeLoss.renderer, 'Renderer rejection followed intentional context loss: ' + rendererRejections.slice(errorsBeforeLoss.renderer).join('\n'));
            assert.equal(fetchedErrorReports.length, errorsBeforeLoss.fetched, 'Error report followed intentional context loss');
            assert.equal(quiet.stats.uncaught.length, errorsBeforeLoss.uncaught, 'Uncaught page error followed intentional context loss');
            if (options.location) {
                locationCounts = await expectLocation('terminal context-loss cleanup', 2, 2, 0, {wait: true});
                assert.deepEqual(locationCounts.clearedIds, [1, 2], 'Terminal cleanup cleared unexpected watches');
            }
            console.log(JSON.stringify(resultSummary(quiet, `${baseMode}+lose-context`, fonts, diagnostics, {
                contextLosses: quiet.stats.contextLosses,
                visibleReloads: quiet.visibleReloads,
                quietMs: LOSS_QUIET_MS,
            })));
        }
        if (options.location) {
            console.log(JSON.stringify({
                status: 'PASS',
                mode: 'consent',
                watches: locationCounts.watchPosition,
                currentRequests: locationCounts.getCurrentPosition,
                clears: locationCounts.clearWatch,
                activeWatches: locationCounts.activeWatches,
            }));
        }
    } finally {
        if (socket?.readyState === WebSocket.OPEN) {
            await send('Browser.close', {}, undefined, 3_000).catch(() => {});
        }
        if (socket && socket.readyState < WebSocket.CLOSING) socket.close();
        rejectPending(new Error('Probe cleanup'));

        let exited = !child || await waitForChildExit(child, closed, 3_000);
        if (!exited && child.pid) {
            child.kill('SIGTERM');
            exited = await waitForChildExit(child, closed, 3_000);
        }
        if (!exited && child.pid) {
            child.kill('SIGKILL');
            exited = await waitForChildExit(child, closed, 3_000);
        }
        if (exited) {
            rmSync(profile, {recursive: true, force: true});
        } else {
            throw new Error(`Owned Chrome process ${child.pid} did not exit; temporary profile retained at ${profile}`);
        }
    }
}

let exitCode = 0;
try {
    await run();
} catch (error) {
    exitCode = 1;
    console.error(error?.stack || String(error));
}
process.exit(exitCode); // Node's CDP WebSocket can otherwise retain an idle event-loop handle.
