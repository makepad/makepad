export async function init(url) {
  let response = await fetch(url);
  let buffer = await response.arrayBuffer();
  let result = await WebAssembly.instantiate(buffer, {
    env: { alert }
  });
  return result.instance.exports;
}
