set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR aarch64)

# Modify these variables with paths to appropriate compilers that can produce
# armv8 targets
set(CMAKE_C_COMPILER aarch64-linux-gnu-gcc)
set(CMAKE_CXX_COMPILER aarch64-linux-gnu-g++)
set(CMAKE_C_COMPILER_AR
    aarch64-linux-gnu-gcc-ar
    CACHE FILEPATH "Archiver")
set(CMAKE_CXX_COMPILER_AR
    aarch64-linux-gnu-gcc-ar
    CACHE FILEPATH "Archiver")
