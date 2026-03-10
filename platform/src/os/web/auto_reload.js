const RETRY_MS = 500;
const decoder = new TextDecoder();

window.makepad_wasm_live_file_change_queue =
    window.makepad_wasm_live_file_change_queue || [];

async function decodeMessage(data) {
    if (typeof data === "string") {
        return data;
    }
    if (data instanceof ArrayBuffer) {
        return decoder.decode(new Uint8Array(data));
    }
    if (ArrayBuffer.isView(data)) {
        return decoder.decode(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    }
    if (data instanceof Blob) {
        return decoder.decode(new Uint8Array(await data.arrayBuffer()));
    }
    return "";
}

function dispatchLiveChange(msg) {
    const hook = window.makepad_wasm_live_file_change;
    if (typeof hook === "function") {
        hook(msg.file_name || "", msg.content || "");
        return;
    }
    window.makepad_wasm_live_file_change_queue.push([
        msg.file_name || "",
        msg.content || "",
    ]);
}

function watchFileChange() {
    const protocol = location.protocol === "https:" ? "wss://" : "ws://";
    const socket = new WebSocket(`${protocol}${location.host}/$watch`);
    socket.binaryType = "arraybuffer";
    socket.onmessage = async (event) => {
        let text = await decodeMessage(event.data);
        if (!text) {
            return;
        }
        let msg = null;
        try {
            msg = JSON.parse(text);
        }
        catch (_error) {
            return;
        }
        if (msg.kind === "live_change") {
            dispatchLiveChange(msg);
            return;
        }
        if (msg.kind === "build_start") {
            console.log("Rebuilding application...");
            return;
        }
        if (msg.kind === "reload") {
            location.href = location.href;
        }
    };
    socket.onclose = () => {
        window.setTimeout(watchFileChange, RETRY_MS);
    };
    socket.onerror = () => {
        socket.close();
    };
}

watchFileChange();
