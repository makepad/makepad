#!/usr/bin/env node

import crypto from 'node:crypto';
import dgram from 'node:dgram';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import tls from 'node:tls';
import zlib from 'node:zlib';

function arg(name, fallback = undefined) {
  const eq = process.argv.find(v => v.startsWith(`--${name}=`));
  if (eq) return eq.slice(name.length + 3);
  const idx = process.argv.indexOf(`--${name}`);
  if (idx >= 0 && idx + 1 < process.argv.length) return process.argv[idx + 1];
  return fallback;
}

const host = arg('host', '10.0.0.182');
const mac = arg('mac', '04-E4-B6-F4-5A-8E');
const pin = arg('pin', '123456');
const displayId = Number(arg('display', '0'));
const timeoutMs = Number(arg('timeout-ms', '120000'));
const imageWidth = Number(arg('width', '800'));
const imageHeight = Number(arg('height', '480'));

function localIp() {
  const explicit = arg('local-ip');
  if (explicit) return explicit;
  const addrs = Object.values(os.networkInterfaces())
    .flat()
    .filter(v => v && v.family === 'IPv4' && !v.internal)
    .map(v => v.address);
  return addrs.find(v => v.startsWith('10.')) || addrs[0] || '127.0.0.1';
}

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) {
      crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function u32be(value) {
  const out = Buffer.alloc(4);
  out.writeUInt32BE(value >>> 0);
  return out;
}

function chunk(type, data = Buffer.alloc(0)) {
  const typeBytes = Buffer.from(type, 'ascii');
  return Buffer.concat([
    u32be(data.length),
    typeBytes,
    data,
    u32be(crc32(Buffer.concat([typeBytes, data]))),
  ]);
}

function randomPng(width, height) {
  const seed = crypto.randomBytes(16);
  const raw = Buffer.alloc((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    const row = y * (width * 4 + 1);
    raw[row] = 0;
    for (let x = 0; x < width; x++) {
      const p = row + 1 + x * 4;
      raw[p] = (x * 3 + y * 7 + seed[0]) & 255;
      raw[p + 1] = (x * 11 + y * 5 + seed[1]) & 255;
      raw[p + 2] = (x * 17 + y * 13 + seed[2]) & 255;
      raw[p + 3] = 255;
    }
  }

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND'),
  ]);
}

function escapeJsonForEmdx(value) {
  return JSON.stringify(value).replaceAll('/', '\\/');
}

function wakeOnLan(targetMac) {
  if (!targetMac) return Promise.resolve();
  const cleaned = targetMac.replace(/[^a-fA-F0-9]/g, '');
  if (cleaned.length !== 12) throw new Error(`Invalid MAC: ${targetMac}`);
  const macBytes = Buffer.from(cleaned, 'hex');
  const packet = Buffer.concat([Buffer.alloc(6, 0xff), ...Array(16).fill(macBytes)]);
  return new Promise((resolve, reject) => {
    const socket = dgram.createSocket('udp4');
    socket.once('listening', () => socket.setBroadcast(true));
    socket.send(packet, 0, packet.length, 9, '255.255.255.255', err => {
      socket.close();
      err ? reject(err) : resolve();
    });
  });
}

function mdcFrame(commandId, id, data) {
  const payload = Buffer.from([commandId, id, data.length, ...data]);
  const checksum = payload.reduce((sum, byte) => sum + byte, 0) % 256;
  return Buffer.from([0xaa, ...payload, checksum]);
}

function parseMdcFrame(buffer) {
  while (buffer.length > 0 && buffer[0] !== 0xaa) buffer = buffer.subarray(1);
  if (buffer.length < 6 || buffer[1] !== 0xff) return [null, buffer];
  const length = buffer[3];
  const frameLength = 5 + length;
  if (buffer.length < frameLength) return [null, buffer];
  const frame = buffer.subarray(0, frameLength);
  const checksum = frame[frame.length - 1];
  const calc = frame.subarray(1, frame.length - 1).reduce((sum, byte) => sum + byte, 0) % 256;
  if (checksum !== calc) return [null, buffer.subarray(1)];
  return [{
    displayId: frame[2],
    ack: frame[4] === 0x41,
    commandId: frame[5],
    payload: frame.subarray(6, 6 + length - 2),
  }, buffer.subarray(frameLength)];
}

function waitForPlainGreeting(socket) {
  return new Promise((resolve, reject) => {
    let text = '';
    const timer = setTimeout(() => reject(new Error('Timed out waiting for MDC TLS greeting')), 10000);
    socket.on('data', data => {
      text += data.toString('utf8');
      if (text.includes('MDCSTART<<TLS>>')) {
        clearTimeout(timer);
        resolve();
      }
    });
    socket.once('error', reject);
  });
}

async function connectMdc() {
  const tcp = net.connect({ host, port: 1515 });
  await waitForPlainGreeting(tcp);

  const secure = await new Promise((resolve, reject) => {
    const socket = tls.connect({ socket: tcp, rejectUnauthorized: false }, () => resolve(socket));
    socket.once('error', reject);
  });

  let text = '';
  let binary = Buffer.alloc(0);
  const pending = [];
  secure.on('data', data => {
    text += data.toString('utf8');
    binary = Buffer.concat([binary, data]);
    let parsed;
    do {
      [parsed, binary] = parseMdcFrame(binary);
      if (parsed) pending.push(parsed);
    } while (parsed);
  });
  secure.write(Buffer.from(pin));

  const start = Date.now();
  while (!text.includes('MDCAUTH<<PASS>>')) {
    if (text.includes('MDCAUTH<<FAIL:0x01>>')) throw new Error('Authentication failed: incorrect PIN');
    if (text.includes('MDCAUTH<<FAIL:0x02>>')) throw new Error('Authentication failed: blocked');
    if (Date.now() - start > 10000) throw new Error('Timed out waiting for MDC auth');
    await new Promise(resolve => setTimeout(resolve, 50));
  }

  return {
    async sendCommand(commandId, data = Buffer.alloc(0)) {
      secure.write(mdcFrame(commandId, displayId, data));
      const start = Date.now();
      while (Date.now() - start < 15000) {
        const idx = pending.findIndex(v => v.commandId === commandId && v.displayId === displayId);
        if (idx >= 0) {
          const [response] = pending.splice(idx, 1);
          if (!response.ack) throw new Error(`MDC NAK command 0x${commandId.toString(16)} payload: ${response.payload.toString('hex')}`);
          return response.payload;
        }
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw new Error(`Timed out waiting for MDC ACK for command 0x${commandId.toString(16)}`);
    },
    async setContentDownload(url) {
      const urlBytes = Buffer.from(url);
      if (urlBytes.length > 255) throw new Error(`URL too long: ${urlBytes.length} bytes`);
      const data = Buffer.from([0x53, 0x80, urlBytes.length, ...urlBytes]);
      await this.sendCommand(0xc7, data);
    },
    close() {
      secure.end();
    },
  };
}

async function printMdcDiagnostics(device) {
  const readString = async (label, commandId) => {
    try {
      const payload = await device.sendCommand(commandId);
      console.log(`[diag] ${label}: ${payload.toString('utf8')} (${payload.toString('hex')})`);
    } catch (err) {
      console.log(`[diag] ${label}: ${err.message}`);
    }
  };

  await readString('serial', 0x0b);
  await readString('software', 0x0e);
  await readString('device name', 0x67);

  const printPayload = async (label, commandId, data = Buffer.alloc(0)) => {
    try {
      const payload = await device.sendCommand(commandId, data);
      console.log(`[diag] ${label}: ${payload.toString('hex')} "${payload.toString('utf8')}"`);
      return payload;
    } catch (err) {
      console.log(`[diag] ${label}: ${err.message}`);
      return null;
    }
  };

  const status = await printPayload('status', 0x00);
  if (status && status.length >= 7) {
    console.log(`[diag] status decoded: power=${status[0]}, volume=${status[1]}, mute=${status[2]}, input=0x${status[3].toString(16)}, aspect=0x${status[4].toString(16)}`);
  }
  const input = await printPayload('input source', 0x14);
  if (input && input.length) {
    console.log(`[diag] input decoded: 0x${input[0].toString(16)}`);
  }
  await printPayload('launcher play via', 0xc7, Buffer.from([0x81]));
  await printPayload('launcher url', 0xc7, Buffer.from([0x82]));
  await printPayload('network config', 0x1b, Buffer.from([0x82]));
  await printPayload('network mode', 0x1b, Buffer.from([0x85]));
  await printPayload('panel', 0xf9);

  try {
    const payload = await device.sendCommand(0x11);
    const state = payload[0] === 0 ? 'Off' : payload[0] === 1 ? 'On' : payload[0] === 2 ? 'Reboot' : `Unknown ${payload[0]}`;
    console.log(`[diag] power: ${state} (${payload.toString('hex')})`);
  } catch (err) {
    console.log(`[diag] power: ${err.message}`);
  }

  try {
    const payload = await device.sendCommand(0x1b, Buffer.from([0x73]));
    console.log(`[diag] battery payload: ${payload.toString('hex')}`);
    if (payload.length >= 7) {
      console.log(`[diag] battery: ${payload[4]}%, plugged=${payload[6] === 0x02}, warning=${payload[2] === 0x01}`);
    }
  } catch (err) {
    console.log(`[diag] battery: ${err.message}`);
  }
}

async function listenOnAvailablePort(server, startPort) {
  for (let port = startPort; port < startPort + 50; port++) {
    try {
      await new Promise((resolve, reject) => {
        server.once('error', reject);
        server.listen(port, '0.0.0.0', resolve);
      });
      return port;
    } catch (err) {
      server.removeAllListeners('error');
      if (err.code !== 'EADDRINUSE') throw err;
    }
  }
  throw new Error(`No available HTTP port near ${startPort}`);
}

function getUrl(url) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, res => {
      const chunks = [];
      res.on('data', chunk => chunks.push(chunk));
      res.on('end', () => resolve({
        statusCode: res.statusCode,
        bytes: Buffer.concat(chunks).length,
      }));
    });
    req.setTimeout(5000, () => {
      req.destroy(new Error(`Timed out fetching ${url}`));
    });
    req.once('error', reject);
  });
}

const ip = localIp();
const fileId = crypto.randomUUID().toUpperCase();
const png = randomPng(imageWidth, imageHeight);
const state = { contentRequests: 0, imageRequests: 0 };
let port;

const server = http.createServer((req, res) => {
  console.log(`[http] ${req.method} ${req.url}`);
  if (req.url.startsWith('/content.json')) {
    state.contentRequests++;
    const content = escapeJsonForEmdx({
      schedule: [{
        start_date: '1970-01-01',
        stop_date: '2999-12-31',
        start_time: '00:00:00',
        contents: [{
          image_url: `http://${ip}:${port}/image`,
          file_id: fileId,
          file_path: `/home/owner/content/Downloads/vxtplayer/epaper/mobile/contents/${fileId}/${fileId}.png`,
          duration: 91326,
          file_size: `${png.length}`,
          file_name: `${fileId}.png`,
        }],
      }],
      name: 'node-samsung-emdx',
      version: 1,
      create_time: '2025-01-01 00:00:00',
      id: fileId,
      program_id: 'com.samsung.ios.ePaper',
      content_type: 'ImageContent',
      deploy_type: 'MOBILE',
    });
    res.writeHead(200, {
      'Content-Type': 'application/json',
      'Content-Length': Buffer.byteLength(content),
      'Cache-Control': 'no-store',
      Connection: 'close',
    });
    res.end(content);
    return;
  }
  if (req.url.startsWith('/image')) {
    state.imageRequests++;
    res.writeHead(200, {
      'Content-Type': 'image/png',
      'Content-Length': png.length,
      'Cache-Control': 'no-store',
      Connection: 'close',
    });
    res.end(png);
    return;
  }
  res.writeHead(404, { 'Content-Type': 'text/plain' });
  res.end('not found');
});

port = await listenOnAvailablePort(server, Number(arg('port', '3000')));
const contentUrl = `http://${ip}:${port}/content.json`;
console.log(`[test] Serving ${png.length} byte PNG as ${fileId}`);
console.log(`[test] Content URL: ${contentUrl}`);
console.log(`[test] Target: ${host}, display ${displayId}`);

try {
  const contentCheck = await getUrl(contentUrl);
  const imageCheck = await getUrl(`http://${ip}:${port}/image`);
  console.log(`[self] content.json ${contentCheck.statusCode}, ${contentCheck.bytes} bytes`);
  console.log(`[self] image ${imageCheck.statusCode}, ${imageCheck.bytes} bytes`);
  state.contentRequests = 0;
  state.imageRequests = 0;

  if (mac) {
    console.log(`[mdc] Wake-on-LAN ${mac}`);
    await wakeOnLan(mac);
    await new Promise(resolve => setTimeout(resolve, 1000));
  }
  console.log('[mdc] Connecting');
  const device = await connectMdc();
  console.log('[mdc] Connected/authenticated');
  await printMdcDiagnostics(device);
  console.log('[mdc] Sending setContentDownload');
  await device.setContentDownload(contentUrl);
  device.close();
  console.log('[mdc] ACK received');

  const start = Date.now();
  while (Date.now() - start < timeoutMs && state.imageRequests === 0) {
    await new Promise(resolve => setTimeout(resolve, 250));
  }

  console.log(`[result] content.json requests: ${state.contentRequests}`);
  console.log(`[result] image requests: ${state.imageRequests}`);
  process.exitCode = state.imageRequests > 0 ? 0 : 2;
} finally {
  server.close();
}
