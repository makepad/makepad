// makepad-web-test's side in the page: `test` and a strict `assert` (node:test's and
// node:assert/strict's, as the test files use them), `read_text` for a file beside the test,
// and the run that loads one test module, runs its tests in order and reports each result to
// the runner (libs/web_test/src/lib.rs) that serves this page.

const cases = [];

/// `test(name, fn)`, `test(name, options, fn)`: registered while the module loads, run in
/// order after it. `fn` may be async; it gets a context with `name` and `diagnostic(text)`.
/// `options.skip` (true or a reason) leaves the test out.
export function test(name, options, fn) {
    if (typeof options === "function") {
        fn = options;
        options = {};
    }
    if (typeof fn !== "function") throw new TypeError(`test ${JSON.stringify(name)} has no function`);
    cases.push({ name: String(name), skip: options?.skip ?? false, fn });
}

/// A file's text, synchronously (the way the test files read their subjects at load).
export function read_text(url) {
    const request = new XMLHttpRequest();
    request.open("GET", String(url), false);
    request.overrideMimeType("text/plain; charset=utf-8");
    request.send();
    if (request.status !== 200) throw new Error(`read_text ${url}: HTTP ${request.status}`);
    return request.responseText;
}

export class AssertionError extends Error {
    constructor(message, { actual, expected, operator } = {}) {
        super(message);
        this.name = "AssertionError";
        this.actual = actual;
        this.expected = expected;
        this.operator = operator;
    }
}

function inspect(value, depth = 0, seen = new Set()) {
    if (typeof value === "string") return JSON.stringify(value);
    if (typeof value === "bigint") return `${value}n`;
    if (Object.is(value, -0)) return "-0";
    if (typeof value === "symbol") return value.toString();
    if (typeof value === "function") return `[Function ${value.name || "(anonymous)"}]`;
    if (value === null || typeof value !== "object") return String(value);
    if (seen.has(value)) return "[Circular]";
    if (value instanceof Error) return `${value.name}: ${value.message}`;
    if (value instanceof Date) return `Date(${value.toISOString?.() ?? value.getTime()})`;
    if (value instanceof RegExp) return String(value);
    if (depth > 4) return Array.isArray(value) ? "[Array]" : "[Object]";
    seen.add(value);
    const inner = (v) => inspect(v, depth + 1, seen);
    let text;
    if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
        text = `${value.constructor.name}(${value.length}) [${Array.from(value, inner).join(", ")}]`;
    } else if (Array.isArray(value)) {
        text = `[${value.map(inner).join(", ")}]`;
    } else if (value instanceof Map) {
        text = `Map(${value.size}) {${[...value].map(([k, v]) => `${inner(k)} => ${inner(v)}`).join(", ")}}`;
    } else if (value instanceof Set) {
        text = `Set(${value.size}) {${[...value].map(inner).join(", ")}}`;
    } else {
        const name = value.constructor && value.constructor !== Object ? `${value.constructor.name} ` : "";
        const keys = [...Object.keys(value), ...Object.getOwnPropertySymbols(value).filter((s) => Object.prototype.propertyIsEnumerable.call(value, s))];
        text = `${name}{${keys.map((k) => `${typeof k === "symbol" ? `[${k.toString()}]` : k}: ${inner(value[k])}`).join(", ")}}`;
    }
    seen.delete(value);
    return text;
}

function enumerable_keys(object) {
    return [...Object.keys(object), ...Object.getOwnPropertySymbols(object).filter((s) => Object.prototype.propertyIsEnumerable.call(object, s))];
}

function same_bytes(a, b) {
    if (a.byteLength !== b.byteLength) return false;
    const x = new Uint8Array(a.buffer ?? a, a.byteOffset ?? 0, a.byteLength);
    const y = new Uint8Array(b.buffer ?? b, b.byteOffset ?? 0, b.byteLength);
    for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return false;
    return true;
}

/// node:assert's deepStrictEqual: primitives by Object.is, the same prototype and type,
/// arrays and objects by their own enumerable keys, typed arrays and buffers by their bytes,
/// maps and sets whatever their order, dates, regular expressions, boxed primitives and
/// errors by their values.
function deep_equal(a, b, memo = new Map()) {
    if (Object.is(a, b)) return true;
    if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
    if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
    const tag = Object.prototype.toString.call(a);
    if (tag !== Object.prototype.toString.call(b)) return false;
    if (memo.get(a) === b) return true;
    memo.set(a, b);
    if (Array.isArray(a) && a.length !== b.length) return false;
    if (a instanceof Date && !Object.is(a.getTime(), b.getTime())) return false;
    if (a instanceof RegExp && (a.source !== b.source || a.flags !== b.flags || a.lastIndex !== b.lastIndex)) return false;
    if (a instanceof Error && (a.message !== b.message || a.name !== b.name)) return false;
    if (a instanceof Number || a instanceof String || a instanceof Boolean || a instanceof BigInt || a instanceof Symbol) {
        if (!Object.is(a.valueOf(), b.valueOf())) return false;
    }
    if (ArrayBuffer.isView(a) || a instanceof ArrayBuffer || (typeof SharedArrayBuffer !== "undefined" && a instanceof SharedArrayBuffer)) {
        if (!same_bytes(a, b)) return false;
        if (ArrayBuffer.isView(a) && !(a instanceof DataView)) {
            // only the keys beside the elements are left to compare
            const extra = (o) => Object.keys(o).filter((k) => !/^\d+$/.test(k));
            const ka = extra(a), kb = extra(b);
            return ka.length === kb.length && ka.every((k) => Object.prototype.hasOwnProperty.call(b, k) && deep_equal(a[k], b[k], memo));
        }
    }
    if (a instanceof Map) {
        if (a.size !== b.size) return false;
        for (const [key, value] of a) {
            if (b.has(key)) {
                if (!deep_equal(value, b.get(key), memo)) return false;
            } else if (typeof key !== "object" || key === null || ![...b].some(([k, v]) => deep_equal(key, k, new Map(memo)) && deep_equal(value, v, new Map(memo)))) {
                return false;
            }
        }
    }
    if (a instanceof Set) {
        if (a.size !== b.size) return false;
        for (const value of a) {
            if (!b.has(value) && (typeof value !== "object" || value === null || ![...b].some((v) => deep_equal(value, v, new Map(memo))))) return false;
        }
    }
    const ka = enumerable_keys(a), kb = enumerable_keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => Object.prototype.propertyIsEnumerable.call(b, k) && deep_equal(a[k], b[k], memo));
}

function fail_with(message, fallback, details) {
    if (message instanceof Error) throw message;
    throw new AssertionError(message ?? fallback, details);
}

function caught(fn) {
    try {
        fn();
    } catch (error) {
        return { error };
    }
    return null;
}

/// Whether `error` is what `expected` describes (node:assert.throws' rules): a RegExp tests
/// its string, an Error class its type, a function returning true accepts it, an object's
/// properties must match (a RegExp property tests the string there).
function matches_expected(error, expected) {
    if (expected instanceof RegExp) return expected.test(String(error));
    if (typeof expected === "function") {
        if (expected.prototype !== undefined && error instanceof expected) return true;
        if (Error.isPrototypeOf(expected) || expected === Error) return false;
        return expected.call({}, error) === true;
    }
    if (typeof expected === "object" && expected !== null) {
        return enumerable_keys(expected).every((key) => {
            const want = expected[key];
            const got = error?.[key];
            return want instanceof RegExp && typeof got === "string" ? want.test(got) : deep_equal(got, want);
        });
    }
    throw new TypeError("assert.throws: `expected` is a RegExp, a function or an object");
}

function ok(value, message) {
    if (!value) fail_with(message, `The expression evaluated to a falsy value: ${inspect(value)}`, { actual: value, expected: true, operator: "==" });
}

/// node:assert/strict: every comparison is strict (Object.is, deepStrictEqual).
export const assert = Object.assign((value, message) => ok(value, message), {
    AssertionError,
    ok,
    equal(actual, expected, message) {
        if (!Object.is(actual, expected)) fail_with(message, `Expected values to be strictly equal:\n\n${inspect(actual)} !== ${inspect(expected)}`, { actual, expected, operator: "strictEqual" });
    },
    notEqual(actual, expected, message) {
        if (Object.is(actual, expected)) fail_with(message, `Expected "actual" to be strictly unequal to: ${inspect(expected)}`, { actual, expected, operator: "notStrictEqual" });
    },
    deepEqual(actual, expected, message) {
        if (!deep_equal(actual, expected)) fail_with(message, `Expected values to be strictly deep-equal:\n+ actual - expected\n\n+ ${inspect(actual)}\n- ${inspect(expected)}`, { actual, expected, operator: "deepStrictEqual" });
    },
    notDeepEqual(actual, expected, message) {
        if (deep_equal(actual, expected)) fail_with(message, `Expected "actual" not to be strictly deep-equal to: ${inspect(expected)}`, { actual, expected, operator: "notDeepStrictEqual" });
    },
    match(string, regexp, message) {
        if (typeof string !== "string" || !regexp.test(string)) fail_with(message, `The input did not match the regular expression ${regexp}. Input:\n\n${inspect(string)}`, { actual: string, expected: regexp, operator: "match" });
    },
    doesNotMatch(string, regexp, message) {
        if (typeof string !== "string" || regexp.test(string)) fail_with(message, `The input was expected to not match the regular expression ${regexp}. Input:\n\n${inspect(string)}`, { actual: string, expected: regexp, operator: "doesNotMatch" });
    },
    throws(fn, expected, message) {
        if (typeof expected === "string") [expected, message] = [undefined, expected];
        const got = caught(fn);
        if (!got) fail_with(message, "Missing expected exception.", { actual: undefined, expected, operator: "throws" });
        if (expected !== undefined && !matches_expected(got.error, expected)) {
            fail_with(message, `The error did not match what was expected (${inspect(expected)}): ${inspect(got.error)}`, { actual: got.error, expected, operator: "throws" });
        }
    },
    doesNotThrow(fn, expected, message) {
        if (typeof expected === "string") message = expected;
        const got = caught(fn);
        if (got) fail_with(message, `Got unwanted exception.\nActual message: "${got.error?.message ?? String(got.error)}"`, { actual: got.error, operator: "doesNotThrow" });
    },
    async rejects(promise_or_fn, expected, message) {
        if (typeof expected === "string") [expected, message] = [undefined, expected];
        try {
            await (typeof promise_or_fn === "function" ? promise_or_fn() : promise_or_fn);
        } catch (error) {
            if (expected !== undefined && !matches_expected(error, expected)) {
                fail_with(message, `The rejection did not match what was expected (${inspect(expected)}): ${inspect(error)}`, { actual: error, expected, operator: "rejects" });
            }
            return;
        }
        fail_with(message, "Missing expected rejection.", { operator: "rejects" });
    },
    async doesNotReject(promise_or_fn, message) {
        try {
            await (typeof promise_or_fn === "function" ? promise_or_fn() : promise_or_fn);
        } catch (error) {
            fail_with(message, `Got unwanted rejection.\nActual message: "${error?.message ?? String(error)}"`, { actual: error, operator: "doesNotReject" });
        }
    },
    fail(message = "Failed") {
        fail_with(message, "Failed");
    },
});
assert.strictEqual = assert.equal;
assert.notStrictEqual = assert.notEqual;
assert.deepStrictEqual = assert.deepEqual;
assert.notDeepStrictEqual = assert.notDeepEqual;
assert.strict = assert;

function describe(error) {
    if (error instanceof Error) return error.stack && error.stack.includes(error.message) ? error.stack : `${error.name}: ${error.message}\n${error.stack ?? ""}`;
    return `uncaught non-error: ${inspect(error)}`;
}

/// Runs one page: `config` is `{ module, report, filters, skip, exact }`, written into the page by
/// the runner. Every event goes to `report` as JSON, one POST each, in order.
export async function run(config) {
    const report = (event) => fetch(config.report, { method: "POST", body: JSON.stringify(event) });
    const uncaught = [];
    window.addEventListener("error", (event) => uncaught.push(describe(event.error ?? event.message)));
    window.addEventListener("unhandledrejection", (event) => uncaught.push(describe(event.reason)));
    const matches = (name, filter) => config.exact ? name === filter : name.includes(filter);
    const selected = (name) => (config.filters.length === 0 || config.filters.some((filter) => matches(name, filter))) && !config.skip.some((skip) => matches(name, skip));
    try {
        await import(config.module);
    } catch (error) {
        await report({ kind: "load_error", error: describe(error) });
        await report({ kind: "done" });
        return;
    }
    await report({ kind: "loaded", count: cases.length });
    const prefix = config.module.split("/").pop().replace(/\.test\.mjs$|\.m?js$/, "");
    for (const entry of cases) {
        const name = `${prefix}::${entry.name}`;
        if (!selected(name)) {
            await report({ kind: "filtered", name });
            continue;
        }
        if (entry.skip) {
            await report({ kind: "ignored", name, error: typeof entry.skip === "string" ? entry.skip : "" });
            continue;
        }
        const diagnostics = [];
        const context = { name: entry.name, diagnostic: (text) => diagnostics.push(String(text)) };
        const started = performance.now();
        let error = "";
        try {
            await entry.fn(context);
        } catch (thrown) {
            error = describe(thrown);
        }
        // what the test left behind: an error thrown from a callback, a rejection nobody took
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (uncaught.length) error = [error, ...uncaught.splice(0).map((text) => `uncaught during the test: ${text}`)].filter(Boolean).join("\n");
        await report({ kind: "result", name, ok: !error, error, diagnostics, ms: performance.now() - started });
    }
    if (uncaught.length) await report({ kind: "load_error", error: uncaught.join("\n") });
    await report({ kind: "done" });
}
