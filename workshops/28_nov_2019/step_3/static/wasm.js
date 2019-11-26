export async function init(url) {
  function consoleLog(data, len) {
    console.log(
      decoder.decode(new Uint8Array(exports.memory.buffer, data, len))
    );
  }

  let decoder = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
  let response = await fetch(url);
  let buffer = await response.arrayBuffer();
  let result = await WebAssembly.instantiate(buffer, {
    env: { console_log: consoleLog }
  });
  let { exports } = result.instance;
  return exports;
}