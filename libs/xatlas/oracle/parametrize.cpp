// Official xatlas oracle: dump Create+AddMesh+Generate (defaults) for a mesh.
// Build: see build.sh. Always compile with XA_MULTITHREADED=0 XA_DEBUG=0 NDEBUG.

#include "../vendor/xatlas.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static void die(const char *msg) {
    std::fprintf(stderr, "oracle: %s\n", msg);
    std::exit(1);
}

static uint32_t f32_bits(float v) {
    uint32_t bits = 0;
    std::memcpy(&bits, &v, 4);
    return bits;
}

struct MeshIn {
    std::vector<float> positions; // xyz xyz ...
    std::vector<uint32_t> indices;
};

static MeshIn load_mesh(const char *path) {
    FILE *f = std::fopen(path, "r");
    if (!f) die("cannot open input mesh");
    char kind[16] = {0};
    if (std::fscanf(f, "%15s", kind) != 1) die("empty mesh file");
    MeshIn mesh;
    if (std::strcmp(kind, "v") == 0) {
        unsigned nv = 0;
        if (std::fscanf(f, "%u", &nv) != 1) die("bad vertex count");
        mesh.positions.resize(size_t(nv) * 3);
        for (unsigned i = 0; i < nv * 3; i++) {
            if (std::fscanf(f, "%f", &mesh.positions[i]) != 1) die("bad vertex");
        }
        if (std::fscanf(f, "%15s", kind) != 1 || std::strcmp(kind, "f") != 0) die("missing f");
        unsigned nf = 0;
        if (std::fscanf(f, "%u", &nf) != 1) die("bad face count");
        mesh.indices.resize(size_t(nf) * 3);
        for (unsigned i = 0; i < nf * 3; i++) {
            if (std::fscanf(f, "%u", &mesh.indices[i]) != 1) die("bad index");
        }
    } else {
        die("expected 'v <count>'");
    }
    std::fclose(f);
    return mesh;
}

int main(int argc, char **argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: xatlas_oracle <mesh.txt>\n");
        return 2;
    }
    MeshIn mesh = load_mesh(argv[1]);
    if (mesh.positions.empty() || mesh.indices.empty()) die("empty mesh");

    xatlas::Atlas *atlas = xatlas::Create();
    xatlas::MeshDecl decl;
    decl.vertexCount = uint32_t(mesh.positions.size() / 3);
    decl.vertexPositionData = mesh.positions.data();
    decl.vertexPositionStride = sizeof(float) * 3;
    decl.indexCount = uint32_t(mesh.indices.size());
    decl.indexData = mesh.indices.data();
    decl.indexFormat = xatlas::IndexFormat::UInt32;
    xatlas::AddMeshError err = xatlas::AddMesh(atlas, decl);
    if (err != xatlas::AddMeshError::Success) {
        std::fprintf(stderr, "oracle: AddMesh failed: %s\n", xatlas::StringForEnum(err));
        return 1;
    }
    xatlas::Generate(atlas);

    if (atlas->meshCount != 1) die("expected one output mesh");
    const xatlas::Mesh &out = atlas->meshes[0];

    std::printf("xatlas-oracle-v1\n");
    std::printf("pin f700c7790aaa030e794b52ba7791a05c085faf0c\n");
    std::printf("flags XA_MULTITHREADED=0 XA_DEBUG=0 NDEBUG\n");
    std::printf("width %u\n", atlas->width);
    std::printf("height %u\n", atlas->height);
    std::printf("atlasCount %u\n", atlas->atlasCount);
    std::printf("chartCount %u\n", atlas->chartCount);
    std::printf("texelsPerUnit %08x\n", f32_bits(atlas->texelsPerUnit));
    std::printf("vertexCount %u\n", out.vertexCount);
    for (uint32_t v = 0; v < out.vertexCount; v++) {
        const xatlas::Vertex &vert = out.vertexArray[v];
        std::printf(
            "v %u %d %d %08x %08x\n",
            vert.xref,
            vert.atlasIndex,
            vert.chartIndex,
            f32_bits(vert.uv[0]),
            f32_bits(vert.uv[1])
        );
    }
    std::printf("indexCount %u\n", out.indexCount);
    for (uint32_t i = 0; i + 2 < out.indexCount; i += 3) {
        std::printf("f %u %u %u\n", out.indexArray[i], out.indexArray[i + 1], out.indexArray[i + 2]);
    }
    xatlas::Destroy(atlas);
    return 0;
}
