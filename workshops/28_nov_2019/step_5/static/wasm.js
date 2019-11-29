export async function init(url) {
  function consoleLog(data, len) {
    console.log(
      decoder.decode(new Uint8Array(exports.memory.buffer, data, len))
    );
  }

  function sierpinski(level) {
    let valuesPtr = exports.sierpinski(level);
    let uint32Memory = new Uint32Array(exports.memory.buffer);
    let value_0 = uint32Memory[valuesPtr / 4 + 0];
    let value_1 = uint32Memory[valuesPtr / 4 + 1];
    let value_2 = uint32Memory[valuesPtr / 4 + 2];
    exports.free_values(valuesPtr);
    return [value_0, value_1, value_2];
  }

  let decoder = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
  let response = await fetch(url);
  let buffer = await response.arrayBuffer();
  let result = await WebAssembly.instantiate(buffer, {
    env: { console_log: consoleLog }
  });
  let { exports } = result.instance;
  return { sierpinski }
}
