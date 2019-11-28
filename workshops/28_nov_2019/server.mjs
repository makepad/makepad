"use strict";

import { promises as fs } from "fs";
import * as http from "http";
import * as path from "path";
import * as url from "url";

const HOST = "127.0.0.1";
const PORT = 8000;

const CONTENT_TYPES = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".wasm": "application/wasm",
};

const server = http
  .createServer(async (request, response) => {
    let filePath = url.parse(request.url).path;
    filePath = filePath.replace(/\/rust_workshop/, ".");

    if (filePath.endsWith("/")) {
      filePath += "index.html";
    }

    let fileExt = path.extname(filePath);
    let contentType = CONTENT_TYPES[fileExt] || "application/octet-stream";

    console.log("Serving file", filePath);

    fs.readFile(filePath)
      .then(data => {
        response.writeHead(200, {
          "Content-Type": contentType,
        });

        response.end(data, "utf-8");
      })
      .catch(error => {
        if (error.code === "ENOENT") {
          response.writeHead(404);
          response.end();
        }
        console.error(error.message);
      });
  })
  .on("error", e => {
    if (e.code === "EADDRINUSE") {
      console.log("Address in use, retrying...");
      setTimeout(() => {
        server.close();
        server.listen(PORT, HOST);
      }, 2000);
    }
  })
  .listen(PORT, HOST);

console.log(`Serving HTTP on ${HOST} port ${PORT}...`);
