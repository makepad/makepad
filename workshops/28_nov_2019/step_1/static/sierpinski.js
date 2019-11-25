import { vec3 } from "./math.js";

export function sierpinski(level) {
  let vertices = [];
  generateTetrix(
    vec3.fromValues(0.0, 0.0, 1.0),
    vec3.fromValues(Math.sqrt(8.0 / 9.0), 0.0, -1.0 / 3.0),
    vec3.fromValues(-Math.sqrt(2.0 / 9.0), Math.sqrt(2.0 / 3.0), -1.0 / 3.0),
    vec3.fromValues(-Math.sqrt(2.0 / 9.0), -Math.sqrt(2.0 / 3.0), -1.0 / 3.0),
    level,
    vertices
  );
  return vertices;
}

function generateTetrix(p0, p1, p2, p3, level, vertices) {
  if (level == 0) {
    generateTriangle(p0, p1, p2, vertices);
    generateTriangle(p0, p2, p3, vertices);
    generateTriangle(p0, p3, p1, vertices);
    generateTriangle(p1, p2, p3, vertices);
  } else {
    let p01 = vec3.lerp(vec3.create(), p0, p1, 0.5);
    let p02 = vec3.lerp(vec3.create(), p0, p2, 0.5);
    let p03 = vec3.lerp(vec3.create(), p0, p3, 0.5);
    let p12 = vec3.lerp(vec3.create(), p1, p2, 0.5);
    let p23 = vec3.lerp(vec3.create(), p2, p3, 0.5);
    let p31 = vec3.lerp(vec3.create(), p3, p1, 0.5);
    generateTetrix(p0, p01, p02, p03, level - 1, vertices);
    generateTetrix(p01, p31, p1, p12, level - 1, vertices);
    generateTetrix(p02, p12, p2, p23, level - 1, vertices);
    generateTetrix(p03, p23, p3, p31, level - 1, vertices);
  }
}

function generateTriangle(p0, p1, p2, vertices) {
  vertices.push(p0[0], p0[1], p0[2]);
  vertices.push(p1[0], p1[1], p1[2]);
  vertices.push(p2[0], p2[1], p2[2]);
}
