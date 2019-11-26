"use strict";

const fs = require("fs");
const http = require("http");
const path = require("path");
const url = require("url");

const HOST = "127.0.0.1";
const PORT = 8000;

const CONTENT_TYPES = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".wasm": "application/wasm"
};

const server = http.createServer((request, response) => {
  let filePath = url.parse(request.url).path;
  filePath = filePath.replace(/\/rust_workshop/, "");
  filePath = "." + filePath;
  if (filePath[filePath.length - 1] == "/") {
    filePath = filePath + "index.html";
  }
  let fileExt = path.extname(filePath);
  let contentType = "application/octet-stream";
  if (fileExt in CONTENT_TYPES) {
    contentType = CONTENT_TYPES[fileExt];
  }
  console.log("Serving file " + filePath);
  fs.readFile(filePath, (error, data) => {
    if (error) {
      if (error.code == "ENOENT") {
        response.writeHead(404);
        response.end();
      }
      console.log(error.message);
      return;
    }
    response.writeHead(200, {
      "Content-Type": contentType
    });
    response.end(data, "utf-8");
  });
});

server.listen(8000, "127.0.0.1", error => {
  if (error != null) {
    console.log(error.message);
    return;
  }
  console.log("Serving HTTP on " + HOST + " port " + PORT + " ...");
});
