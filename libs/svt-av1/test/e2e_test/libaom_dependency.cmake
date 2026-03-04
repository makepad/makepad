# We can't easily use FetchContent because aom's CMakeLists.txt adds a bunch of flags that we don't want to deal with
include(ExternalProject)

# This check is to account for the fact that you don't link to the shared dll
# Windows, but instead the import lib.
if(BUILD_SHARED_LIBS AND NOT MSVC)
    set(LIB_PREFIX ${CMAKE_SHARED_LIBRARY_PREFIX})
else()
    set(LIB_PREFIX ${CMAKE_STATIC_LIBRARY_PREFIX})
endif()

# Use the configuration-specific directory so multi-config generators are
# handled
set(AOM_LIB
    "${CMAKE_ARCHIVE_OUTPUT_DIRECTORY}/${CMAKE_CFG_INTDIR}/${LIB_PREFIX}aom${CMAKE_STATIC_LIBRARY_SUFFIX}"
)

ExternalProject_Add(
    DepLibAom
    UPDATE_DISCONNECTED True
    PREFIX "${CMAKE_BINARY_DIR}/libaom"
    GIT_REPOSITORY "https://aomedia.googlesource.com/aom"
    GIT_TAG av1-normative
    GIT_SHALLOW 1
    CMAKE_ARGS
        ${CUSTOM_CONFIG}
        -DCONFIG_INSPECTION=1
        -DENABLE_TESTS=0
        -DCONFIG_AV1_ENCODER=0
        -DENABLE_DOCS=0
        -DENABLE_EXAMPLES=0
        -DENABLE_TESTDATA=0
        -DENABLE_TOOLS=0
        -DCMAKE_ARCHIVE_OUTPUT_DIRECTORY=${CMAKE_ARCHIVE_OUTPUT_DIRECTORY}
        -DCMAKE_LIBRARY_OUTPUT_DIRECTORY=${CMAKE_LIBRARY_OUTPUT_DIRECTORY}
        -DCMAKE_RUNTIME_OUTPUT_DIRECTORY=${CMAKE_RUNTIME_OUTPUT_DIRECTORY}
        -DCMAKE_TOOLCHAIN_FILE=${CMAKE_TOOLCHAIN_FILE}
    BUILD_BYPRODUCTS ${AOM_LIB}
    INSTALL_COMMAND "")

if(NOT TARGET aom)
    add_library(aom STATIC IMPORTED GLOBAL)
    set_target_properties(aom PROPERTIES IMPORTED_LOCATION "${AOM_LIB}")
    add_dependencies(aom DepLibAom)
endif()
