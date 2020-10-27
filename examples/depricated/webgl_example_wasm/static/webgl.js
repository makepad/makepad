import {mat4, vec3} from "./math.js";

const VERTEX_SHADER_SOURCE = `
uniform mat4 uModelViewMatrix;

attribute vec3 aPosition;

varying vec3 vColor;

void main() {
    gl_Position = uModelViewMatrix * vec4(aPosition, 1.0);
    vColor = aPosition * 0.5 + 0.5;
}
 `;

const FRAGMENT_SHADER_SOURCE = `
precision highp float;

varying vec3 vColor;

void main() {
    gl_FragColor = vec4(vColor, 1.0);
}
 `;

export function init(gl, vertices) {
    function render(currentTime) {
        let deltaTime = currentTime - previousTime;
        previousTime = currentTime;
        mat4.rotate(
            modelViewMatrix,
            modelViewMatrix,
            (deltaTime * 0.2 * Math.PI) / 1000.0,
            vec3.fromValues(1.0, 2.0, 3.0)
        );
        gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
        gl.useProgram(program);
        gl.uniformMatrix4fv(modelViewMatrixUniform, false, modelViewMatrix);
        gl.bindBuffer(gl.ARRAY_BUFFER, vertexBuffer);
        gl.enableVertexAttribArray(positionAttribute);
        gl.vertexAttribPointer(positionAttribute, 3, gl.FLOAT, gl.FALSE, 0, 0);
        gl.drawArrays(gl.TRIANGLES, 0, vertices.length / 3);
        gl.bindBuffer(gl.ARRAY_BUFFER, null);
        gl.useProgram(null);
    }
    
    function linkProgram(vertexShader, fragmentShader) {
        let program = gl.createProgram();
        gl.attachShader(program, vertexShader);
        gl.attachShader(program, fragmentShader);
        gl.linkProgram(program);
        if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
            throw new Error(gl.getProgramInfoLog(program));
        }
        return program;
    }
    
    function compileShader(type, source) {
        let shader = gl.createShader(type);
        gl.shaderSource(shader, source);
        gl.compileShader(shader);
        if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
            throw new Error(gl.getShaderInfoLog(shader));
        }
        return shader;
    }
    
    gl.clearColor(0.0, 0.0, 0.0, 1.0);
    gl.enable(gl.DEPTH_TEST);
    let program = linkProgram(
        compileShader(gl.VERTEX_SHADER, VERTEX_SHADER_SOURCE),
        compileShader(gl.FRAGMENT_SHADER, FRAGMENT_SHADER_SOURCE)
    );
    let modelViewMatrixUniform = gl.getUniformLocation(
        program,
        "uModelViewMatrix"
    );
    let positionAttribute = gl.getAttribLocation(program, "aPosition");
    let vertexBuffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vertexBuffer);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(vertices), gl.STATIC_DRAW);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    let previousTime = performance.now();
    let modelViewMatrix = mat4.create();
    return {render};
}
