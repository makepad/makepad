export async function init(url) {
  function consoleLog(data, len) {
    console.log(
      decoder.decode(new Uint8Array(exports.memory.buffer, data, len))
    );
  }

  function sierpinski(level) {
    let rawPartsPtr = exports.sierpinski(level);
    let int32Memory = new Int32Array(exports.memory.buffer);
    let ptr = int32Memory[rawPartsPtr / 4 + 0];
    let len = int32Memory[rawPartsPtr / 4 + 1];
    let capacity = int32Memory[rawPartsPtr / 4 + 2];
    let float32Memory = new Float32Array(exports.memory.buffer);
    let result = float32Memory.subarray(ptr / 4, ptr / 4 + len).slice();
    exports.free_vec_f32(rawPartsPtr);
    return result;
  }

  let decoder = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
  let response = await fetch(url);
  let buffer = await response.arrayBuffer();
  let result = await WebAssembly.instantiate(buffer, {
    env: { console_log: consoleLog }
  });
  let { exports } = result.instance;
  return { sierpinski };
}
