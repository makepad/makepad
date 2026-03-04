#
# Copyright(c) 2018 Intel Corporation
#
# This source code is subject to the terms of the BSD 2 Clause License and
# the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
# was not distributed with this source code in the LICENSE file, you can
# obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
# Media Patent License 1.0 was not distributed with this source code in the
# PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
#

cmake_minimum_required(VERSION 3.16...4.1)

if("${CMAKE_CURRENT_SOURCE_DIR}" STREQUAL "${CMAKE_CURRENT_BINARY_DIR}")
    message(WARNING "Building in-source is highly not recommended\n"
                    "Please use the Build folder or create your own.")
endif()

project(svt-av1 VERSION 4.0.1
    LANGUAGES C CXX)

set(VERSION_TAG "")

set(CMAKE_C_VISIBILITY_PRESET hidden)
# Default build type to release if the generator does not has its own set of build types
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE Release)
endif()

if(NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(WARNING "32-bit is not supported")
endif()

include(CheckCSourceCompiles)

check_c_source_compiles("
#if defined(_M_X64) || defined(__x86_64__)
#else
#error \"Non-x64\"
#endif
int main(void) {}
" HAVE_X86_PLATFORM)

check_c_source_compiles("
#if defined(__aarch64__) || defined(_M_ARM64)
#else
#error \"Non-arm64\"
#endif
int main(void) {}
" HAVE_ARM_PLATFORM)

option(COMPILE_C_ONLY "Compile only C code with no simds (autodetect, default off for x86)" OFF)
option(MINIMAL_BUILD "Enable minimal build" OFF)
if(MINIMAL_BUILD)
    add_definitions(-DMINIMAL_BUILD=1)
endif()

option(LOG_QUIET "Disable all logging from encoder" OFF)
if(LOG_QUIET)
    add_definitions(-DCONFIG_LOG_QUIET=1)
endif()

option(RTC_BUILD "Optimize build for RTC by disabling unused features and logging" OFF)
if(RTC_BUILD)
    add_definitions(-DRTC_BUILD=1)
endif()

if(NOT COMPILE_C_ONLY AND HAVE_X86_PLATFORM)
    include(CheckLanguage)
    check_language(ASM_NASM)
    if(CMAKE_ASM_NASM_COMPILER MATCHES "nasm")
        set(NASM_VERSION "0.0.0")
        execute_process(COMMAND ${CMAKE_ASM_NASM_COMPILER} -v
            OUTPUT_VARIABLE NASM_VERSION
            ERROR_QUIET
            OUTPUT_STRIP_TRAILING_WHITESPACE)
        string(REGEX MATCH "([.0-9])+" NASM_VERSION "${NASM_VERSION}")
        # NASM_VERSION should now contain something like 2.14.02
        # Needs to error out on a version less than 2.14
        if(NASM_VERSION VERSION_LESS "2.14")
            find_program(YASM_EXE yasm)
            if(YASM_EXE)
                set(CMAKE_ASM_NASM_COMPILER ${YASM_EXE})
                message(STATUS "Found nasm is too old (requires at least 2.14, found ${NASM_VERSION}), using yasm instead")
            else()
                message(FATAL_ERROR "Found nasm is too old (requires at least 2.14, found ${NASM_VERSION})!")
            endif()
        endif()
    endif()
    enable_language(ASM_NASM)
    add_definitions(-DARCH_X86_64=1)
elseif(NOT COMPILE_C_ONLY AND HAVE_ARM_PLATFORM)
    enable_language(ASM) # needed for .S files
    set(ARM64_FLAVORS "NEON;ARM_CRC32;NEON_DOTPROD;NEON_I8MM;SVE;SVE2")
    set(ARM_CRC32_DEFAULT_FLAG "-march=armv8-a+crc")
    set(NEON_DOTPROD_DEFAULT_FLAG "-march=armv8.2-a+dotprod")
    set(NEON_I8MM_DEFAULT_FLAG "-march=armv8.2-a+dotprod+i8mm")
    set(SVE_DEFAULT_FLAG "-march=armv8.2-a+dotprod+i8mm+sve")
    set(SVE2_DEFAULT_FLAG "-march=armv9-a+i8mm+sve2") # SVE2 is a v9-only feature

    option(ENABLE_NEON "Enables Neon optimizations on AArch64 targets." ON)
    option(ENABLE_ARM_CRC32 "Enables Arm CRC32 optimizations." ON)
    option(ENABLE_NEON_DOTPROD
      "Enables Armv8.2-A Neon DotProd optimizations on AArch64 targets." ON)
    option(ENABLE_NEON_I8MM
      "Enables Armv8.2-A Neon I8MM optimizations on AArch64 targets." ON)
    option(ENABLE_SVE "Enables Armv8.2-A SVE optimizations on AArch64 targets." ON)
    option(ENABLE_SVE2 "Enables Armv9-A SVE2 optimizations on AArch64 targets." ON)

    # Check that the compiler flag to enable each flavor is supported by the
    # compiler. This may not be the case for new architecture features on old
    # compiler versions.
    set(C_MAIN "\nint main(void) { return 0; }")
    foreach(flavor ${ARM64_FLAVORS})
        if(ENABLE_${flavor} AND NOT DEFINED ${flavor}_FLAG)
            set(${flavor}_FLAG "${${flavor}_DEFAULT_FLAG}")

            # Do not use check_c_compiler_flag here since the regex used to match
            # against stderr does not recognise the "invalid feature modifier" error
            # produced by certain versions of GCC, leading to the feature being
            # incorrectly marked as available.
            set(OLD_CMAKE_REQUIRED_FLAGS ${CMAKE_REQUIRED_FLAGS})
            set(CMAKE_REQUIRED_FLAGS "${CMAKE_REQUIRED_FLAGS} ${${flavor}_FLAG}")
            unset(${flavor}_FLAG_SUPPORTED)
            check_c_source_compiles("static void function(void) {} ${C_MAIN}" ${flavor}_FLAG_SUPPORTED)
            set(CMAKE_REQUIRED_FLAGS ${OLD_CMAKE_REQUIRED_FLAGS})

            if(NOT ${flavor}_FLAG_SUPPORTED)
              set(ENABLE_${flavor} 0)
            endif()
        endif()
    endforeach()

    # SVE and SVE2 require that the Neon-SVE bridge header is also available.
    if(ENABLE_SVE OR ENABLE_SVE2)
        set(OLD_CMAKE_REQUIRED_FLAGS ${CMAKE_REQUIRED_FLAGS})
        set(OLD_CMAKE_TRY_COMPILE_TARGET_TYPE ${CMAKE_TRY_COMPILE_TARGET_TYPE})
        set(CMAKE_REQUIRED_FLAGS "${CMAKE_REQUIRED_FLAGS} ${SVE_FLAG}")
        set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
        check_c_source_compiles("
#ifndef __ARM_NEON_SVE_BRIDGE
#error 1
#endif
#include <arm_sve.h>
#include <arm_neon_sve_bridge.h>" HAVE_SVE_HEADERS)
        # Check whether the compiler can compile SVE functions that require
        # backup/restore of SVE registers according to AAPCS. Clang for Windows used
        # to fail this, see https://github.com/llvm/llvm-project/issues/80009.
        check_c_source_compiles("
#include <arm_sve.h>
void other(void);
svfloat32_t func(svfloat32_t a) {
  other();
  return a;
} ${C_MAIN}" CAN_COMPILE_SVE)
        set(CMAKE_REQUIRED_FLAGS ${OLD_CMAKE_REQURED_FLAGS})
        set(CMAKE_TRY_COMPILE_TARGET_TYPE ${OLD_CMAKE_TRY_COMPILE_TARGET_TYPE})
        if(NOT HAVE_SVE_HEADERS OR NOT CAN_COMPILE_SVE)
            set(ENABLE_SVE 0)
            set(ENABLE_SVE2 0)
        endif()
    endif()

    foreach(flavor ${ARM64_FLAVORS})
        if(ENABLE_${flavor})
            add_compile_definitions(HAVE_${flavor}=1)
        else()
            add_compile_definitions(HAVE_${flavor}=0)
        endif()
    endforeach()

    add_definitions(-DARCH_AARCH64=1)
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -fno-lax-vector-conversions")
    # Prevents missmatch between arm and x86 bitstreams
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -ffp-contract=off")
endif()

include(GNUInstallDirs)
include(CheckCCompilerFlag)
include(CheckCXXCompilerFlag)

# if(PROJECT_IS_TOP_LEVEL) requires CMake 3.21
if(CMAKE_SOURCE_DIR STREQUAL PROJECT_SOURCE_DIR)
    set(CMAKE_OUTPUT_DIRECTORY "${PROJECT_SOURCE_DIR}/Bin/${CMAKE_BUILD_TYPE}/" CACHE PATH "Location to write the resulting binaries to")

    set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY ${CMAKE_OUTPUT_DIRECTORY})
    set(CMAKE_LIBRARY_OUTPUT_DIRECTORY ${CMAKE_OUTPUT_DIRECTORY})
    set(CMAKE_RUNTIME_OUTPUT_DIRECTORY ${CMAKE_OUTPUT_DIRECTORY})
endif()

set(PC_LIBS_PRIVATE)
set(PC_REQUIRES_PRIVATE)

#Clang support, required to build static with LTO
if(CMAKE_C_COMPILER_ID MATCHES "Clang" AND UNIX AND NOT APPLE)
    find_program(LLVM_LD_EXE llvm-ld)
    find_program(LLVM_AR_EXE llvm-ar)
    find_program(LLVM_RANLIB_EXE llvm-ranlib)
    if(LLVM_LD_EXE)
        SET (CMAKE_LINKER  "llvm-ld")
    endif()
    if(LLVM_AR_EXE)
        SET (CMAKE_AR      "llvm-ar")
    endif()
    if(LLVM_RANLIB_EXE)
        SET (CMAKE_RANLIB  "llvm-ranlib")
    endif()
endif()

if(MSVC)
    if(DEFINED SANITIZER)
        message(WARNING "Sanitizers are not available with MSVC currently")
    endif()
    unset(SANITIZER)
    unset(SANITIZER CACHE)
else()
    set(SANITIZER "" CACHE STRING "Sanitizer to enable. Currently known values are Address, Memory, Thread, Undefined, and Integer")
    set_property(CACHE SANITIZER PROPERTY STRINGS Address Memory Thread Undefined Integer)
endif()

# Always build with -fPIC
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
set(CMAKE_C_STANDARD 99)
set(CMAKE_CXX_STANDARD 11)

# BUILD_SHARED_LIBS is a standard CMake variable, but we declare it here to
# make it prominent in the GUI.
option(BUILD_SHARED_LIBS "Build shared libraries (DLLs)." ON)
message(STATUS "BUILD_SHARED_LIBS: ${BUILD_SHARED_LIBS}")

option(BUILD_TESTING "Build SvtAv1UnitTests, SvtAv1ApiTests, and SvtAv1E2ETests unit tests")
option(COVERAGE "Generate coverage report")
if(NOT MSVC)
    option(SVT_AV1_PGO "Enable profile guided optimization. Creates the RunPGO and CompilePGO targets")
    # options for internal use
    option(SVT_INTERNAL_PGO_GENERATE "Generate a profile guided optimization profile")
    option(SVT_INTERNAL_PGO_USE "Use a profile for profile guided optimization")
    mark_as_advanced(SVT_INTERNAL_PGO_GENERATE SVT_INTERNAL_PGO_USE)
    set(SVT_AV1_PGO_DIR "${CMAKE_BINARY_DIR}" CACHE PATH "Path to the directory passed to -fprofile-dir for use with profile guided optimization")
    set(SVT_AV1_PGO_CUSTOM_VIDEOS "" CACHE STRING "Semi-colon separated list of directories with y4m videos to run PGO on instead of objective-1-fast")
else()
    set(SVT_AV1_PGO OFF)
endif()

if(MSVC AND SVT_AV1_PGO)
    message(WARNING "PGO is not yet supported for MSVC, disabling")
    unset(SVT_AV1_PGO)
    unset(SVT_AV1_PGO CACHE)
endif()

if(CMAKE_C_COMPILER_ID MATCHES "Clang" AND SVT_AV1_PGO)
    find_program(LLVM_PROFDATA llvm-profdata)
    if(NOT LLVM_PROFDATA)
        message(FATAL_ERROR "llvm-profdata not found.")
    endif()
endif()

option(BUILD_APPS "Build Enc Apps" ON)
option(EXCLUDE_HASH "Disables the use of the git hash in the build to produce hash exact binaries, implies REPRODUCIBLE_BUILDS")
option(REPRODUCIBLE_BUILDS "Disable use of certain C macros to always generate reproducable builds" ${EXCLUDE_HASH})
if(SVT_AV1_PGO AND NOT BUILD_APPS)
    message(FATAL_ERROR "Can't use pgo and not build the apps")
endif()

if(SVT_INTERNAL_PGO_USE)
    if(MSVC)
        set(PGO_COMPILER_FLAGS "/GL")
        set(PGO_LINKER_FLAGS "/LTCG /USEPROFILE")
    else()
        set(PGO_COMPILER_FLAGS "-fprofile-use=${SVT_AV1_PGO_DIR}")
        set(PGO_LINKER_FLAGS "-fprofile-use=${SVT_AV1_PGO_DIR}")
        if (CMAKE_C_COMPILER_ID STREQUAL "GNU") # GCC
            # We want unprofiled code optimised for speed, not size, so we mark the profiling data as "partial".
            set(PGO_COMPILER_FLAGS "${PGO_COMPILER_FLAGS} -fprofile-correction -fprofile-partial-training")
            set(PGO_LINKER_FLAGS "${PGO_LINKER_FLAGS} -fprofile-correction -fprofile-partial-training")
        endif()
    endif()
elseif(SVT_INTERNAL_PGO_GENERATE)
    if(MSVC)
        set(PGO_COMPILER_FLAGS "/GL")
        set(PGO_LINKER_FLAGS "/LTCG /GENPROFILE")
    else()
        set(PGO_COMPILER_FLAGS "-fprofile-generate=${SVT_AV1_PGO_DIR}")
        set(PGO_LINKER_FLAGS "-fprofile-generate=${SVT_AV1_PGO_DIR}")
    endif()
endif()

if(SVT_AV1_PGO)
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} ${PGO_COMPILER_FLAGS}")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} ${PGO_COMPILER_FLAGS}")
    set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} ${PGO_LINKER_FLAGS}")
    set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} ${PGO_LINKER_FLAGS}")
    set(CMAKE_MODULE_LINKER_FLAGS "${CMAKE_MODULE_LINKER_FLAGS} ${PGO_LINKER_FLAGS}")
endif()

if(EXCLUDE_HASH)
    add_link_options(-Wl,--build-id=none)
    add_definitions(-DEXCLUDE_HASH=1)
else()
    add_definitions(-DEXCLUDE_HASH=0)
endif()

if(REPRODUCIBLE_BUILDS)
    add_definitions(-DREPRODUCIBLE_BUILDS=1)
else()
    add_definitions(-DREPRODUCIBLE_BUILDS=0)
endif()


if(WIN32)
    set(CMAKE_ASM_NASM_FLAGS "${CMAKE_ASM_NASM_FLAGS} -DWIN64")
else()
    set(CMAKE_ASM_NASM_FLAGS "${CMAKE_ASM_NASM_FLAGS} -DUNIX64")
endif()

if(UNIX)
    if(APPLE)
        set(CMAKE_MACOSX_RPATH 1)
        if(CMAKE_C_COMPILER_ID MATCHES "AppleClang")
            set(CMAKE_C_ARCHIVE_CREATE   "<CMAKE_AR> Scr <TARGET> <LINK_FLAGS> <OBJECTS>")
            set(CMAKE_CXX_ARCHIVE_CREATE "<CMAKE_AR> Scr <TARGET> <LINK_FLAGS> <OBJECTS>")
            set(CMAKE_C_ARCHIVE_FINISH   "<CMAKE_RANLIB> -no_warning_for_no_symbols -c <TARGET>")
            set(CMAKE_CXX_ARCHIVE_FINISH "<CMAKE_RANLIB> -no_warning_for_no_symbols -c <TARGET>")
        endif()
    elseif(NOT EMSCRIPTEN)
        set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} -z noexecstack -z relro -z now")
    endif()
endif()

function(check_flag lang flag)
    string(REGEX REPLACE "[^A-Za-z0-9]" "_" flag_var "${flag}")
    if(NOT DEFINED ${lang}_FLAG${flag_var})
        execute_process(COMMAND ${CMAKE_COMMAND} -E echo_append "-- Checking ${lang} flag support for: [${flag}] - ")
        if(CMAKE_C_COMPILER_ID MATCHES "Clang")
            set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -Werror=unused-command-line-argument")
            set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -Werror=unused-command-line-argument")
        endif()
        if("${lang}" STREQUAL "CXX")
            check_cxx_compiler_flag("${flag}" "${lang}_FLAG${flag_var}")
        else()
            check_c_compiler_flag("${flag}" "${lang}_FLAG${flag_var}")
        endif()
        if(${lang}_FLAG${flag_var})
            execute_process(COMMAND ${CMAKE_COMMAND} -E echo "Yes")
        else()
            execute_process(COMMAND ${CMAKE_COMMAND} -E echo "No")
        endif()
    endif()
    set(check_flag ${${lang}_FLAG${flag_var}} PARENT_SCOPE)
endfunction()

function(check_flags_add lang)
    cmake_parse_arguments(check_${lang}_flags_add
        "PREPEND"
        "TYPE"
        ""
        ${ARGN})

    if(check_${lang}_flags_add_TYPE)
        set(check_${lang}_flags_add_TYPE "_${check_${lang}_flags_add_TYPE}")
    endif()
    if(check_${lang}_flags_add_PREPEND)
        list(REVERSE check_${lang}_flags_add_UNPARSED_ARGUMENTS)
    endif()

    set(CMAKE_REQUIRED_QUIET true)

    foreach(flag IN LISTS check_${lang}_flags_add_UNPARSED_ARGUMENTS)
        check_flag("${lang}" "${flag}")
        if(NOT check_flag)
            continue()
        endif()

        if(check_${lang}_flags_add_PREPEND)
            set(CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE} "${flag} ${CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE}}")
        else()
            set(CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE} "${CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE}} ${flag}")
        endif()
    endforeach()
    if(CMAKE_BUILD_TYPE MATCHES "[Dd][Ee][Bb][Uu][Gg]")
        message(STATUS "${CMAKE_CURRENT_SOURCE_DIR} ${lang}FLAGS: ${CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE}}")
    endif()
    set(CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE} "${CMAKE_${lang}_FLAGS${check_${lang}_flags_add_TYPE}}" PARENT_SCOPE)
endfunction()

macro(check_both_flags_add)
    check_flags_add(C "${ARGV}")
    check_flags_add(CXX "${ARGV}")
endmacro()

macro(add_clike_and_ld_flags)
    string(REPLACE ";" " " add_clike_and_ld_flags "${ARGV}")
    set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} ${add_clike_and_ld_flags}")
    set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} ${add_clike_and_ld_flags}")
    set(CMAKE_EXE_LINKER_FLAGS "${CMAKE_EXE_LINKER_FLAGS} ${add_clike_and_ld_flags}")
    set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} ${add_clike_and_ld_flags}")
endmacro()

if((CMAKE_C_COMPILER_ID STREQUAL "GNU" AND CMAKE_C_COMPILER_VERSION VERSION_GREATER_EQUAL "9.0") OR
   (CMAKE_C_COMPILER_ID STREQUAL "Clang" AND CMAKE_C_COMPILER_VERSION VERSION_GREATER_EQUAL "12.0"))
    option(SVT_AV1_LTO "Attempt to enable Link Time Optimization if available" ON)
else()
    option(SVT_AV1_LTO "Attempt to enable Link Time Optimization if available" OFF)
endif()
if(SVT_AV1_LTO)
    include(CheckIPOSupported)
    check_ipo_supported(RESULT svt_av1_ipo_supported)
    if(svt_av1_ipo_supported AND NOT DEFINED CMAKE_INTERPROCEDURAL_OPTIMIZATION)
        set(CMAKE_INTERPROCEDURAL_OPTIMIZATION ON)
    endif()
endif()

check_both_flags_add(PREPEND -Wextra -Wformat -Wformat-security)

if(MSVC)
    check_both_flags_add(PREPEND /W3)
    check_both_flags_add(/MP)
    check_both_flags_add(TYPE DEBUG /Od)
else()
    check_both_flags_add(PREPEND -Wall)
    option(NATIVE "Build for native performance (march=native)" OFF)
    if(NATIVE)
        check_both_flags_add(-march=native)
    endif()
    if(MINGW)
        check_flags_add(C -mxsave -fno-asynchronous-unwind-tables)
    else()
        check_both_flags_add(-fstack-protector-strong)
        if(NOT EMSCRIPTEN)
            check_both_flags_add(PREPEND -fno-stack-clash-protection)
        endif()
    endif()
    check_both_flags_add(-mno-avx)
endif()

if(CMAKE_C_FLAGS MATCHES "-O" AND NOT CMAKE_C_FLAGS MATCHES "-O0" AND NOT MINGW)
    add_definitions(-D_FORTIFY_SOURCE=2)
endif()

if(CMAKE_ASM_NASM_OBJECT_FORMAT MATCHES "win")
    set(CMAKE_ASM_NASM_FLAGS_DEBUG "${CMAKE_ASM_NASM_FLAGS_DEBUG} -gcv8")
elseif(CMAKE_ASM_NASM_COMPILER MATCHES "nasm")
    set(CMAKE_ASM_NASM_FLAGS_DEBUG "${CMAKE_ASM_NASM_FLAGS_DEBUG} -gdwarf")
elseif(CMAKE_ASM_NASM_COMPILER MATCHES "yasm")
    if(CMAKE_ASM_NASM_OBJECT_FORMAT MATCHES "macho")
        set(CMAKE_ASM_NASM_FLAGS_DEBUG "${CMAKE_ASM_NASM_FLAGS_DEBUG} -gnull")
    else()
        set(CMAKE_ASM_NASM_FLAGS_DEBUG "${CMAKE_ASM_NASM_FLAGS_DEBUG} -gdwarf2")
    endif()
endif()

include(CheckSymbolExists)
include(CheckCCompilerFlag)
if(MSVC)
check_c_compiler_flag("/arch:AVX512" HAS_AVX512)
else()
check_c_compiler_flag("-mavx512f" HAS_AVX512)
endif()

if(HAS_AVX512)
    option(ENABLE_AVX512 "Enable building avx512 code" ON)
else()
    set(ENABLE_AVX512 OFF CACHE INTERNAL "Enable building avx512 code")
endif()
if(ENABLE_AVX512)
    add_definitions(-DEN_AVX512_SUPPORT=1)
else()
    add_definitions(-DEN_AVX512_SUPPORT=0)
endif()

# ASM compiler macro
macro(ASM_COMPILE_TO_TARGET target)
    if(CMAKE_GENERATOR STREQUAL "Xcode")
        if(CMAKE_BUILD_TYPE MATCHES Release)
            set(ASM_FLAGS "${CMAKE_ASM_NASM_FLAGS_RELEASE}")
        elseif(CMAKE_BUILD_TYPE MATCHES Debug)
            set(ASM_FLAGS "${CMAKE_ASM_NASM_FLAGS_DEBUG}")
        elseif(CMAKE_BUILD_TYPE MATCHES MinSizeRel)
            set(ASM_FLAGS "${CMAKE_ASM_NASM_FLAGS_MINSIZEREL}")
        elseif(CMAKE_BUILD_TYPE MATCHES RelWithDebInfo)
            set(ASM_FLAGS "${CMAKE_ASM_NASM_FLAGS_RELWITHDEBINFO}")
        endif()

        get_property(inc_dirs DIRECTORY ${CMAKE_CURRENT_SOURCE_DIR} PROPERTY INCLUDE_DIRECTORIES)
        foreach(inc_dir ${inc_dirs})
            set(ASM_FLAGS "${ASM_FLAGS} -I${inc_dir}")
        endforeach()

        string(REGEX REPLACE " " ";" ASM_FLAGS "${ASM_FLAGS} ${CMAKE_ASM_NASM_FLAGS}")

        foreach(asm_file ${ARGN})
            get_filename_component(filename "${asm_file}" NAME_WE)
            set(SIMD_OBJ "${CMAKE_CURRENT_BINARY_DIR}/${filename}${CMAKE_C_OUTPUT_EXTENSION}")
            message(STATUS "Pregenerating ASM_NASM file ${filename}${CMAKE_C_OUTPUT_EXTENSION}")
            execute_process(COMMAND ${CMAKE_ASM_NASM_COMPILER}
                    -f${CMAKE_ASM_NASM_OBJECT_FORMAT}
                    -I${CMAKE_CURRENT_SOURCE_DIR}
                    -o${SIMD_OBJ}
                    ${ASM_FLAGS}
                    ${CMAKE_CURRENT_SOURCE_DIR}/${asm_file}
                    WORKING_DIRECTORY ${CMAKE_CURRENT_LIST_DIR})
            list(APPEND SIMD_OBJS "${SIMD_OBJ}")
        endforeach()
        target_sources(common_lib INTERFACE ${SIMD_OBJS})
    else()
        # For every other generator
        foreach(asm_file ${ARGN})
            get_filename_component(filename "${asm_file}" ABSOLUTE)
            target_sources(${target} PUBLIC ${filename})
        endforeach()
    endif()
endmacro()

check_symbol_exists(strnlen_s "string.h" HAVE_STRNLEN_S)
check_symbol_exists(strncpy_s "string.h" HAVE_STRNCPY_S)
check_symbol_exists(strcpy_s "string.h" HAVE_STRCPY_S)
set_property(DIRECTORY .
    APPEND
    PROPERTY COMPILE_DEFINITIONS
        SAFECLIB_STR_NULL_SLACK=1
        $<$<BOOL:${HAVE_STRNLEN_S}>:HAVE_STRNLEN_S=1>
        $<$<BOOL:${HAVE_STRNCPY_S}>:HAVE_STRNCPY_S=1>
        $<$<BOOL:${HAVE_STRCPY_S}>:HAVE_STRCPY_S=1>
        $<$<BOOL:${WIN32}>:_WIN32_WINNT=0x0601>)
if(NOT HAVE_STRCPY_S OR NOT HAVE_STRNCPY_S OR NOT HAVE_STRNLEN_S)
    add_library(safestringlib OBJECT
        third_party/safestringlib/safeclib_private.h
        third_party/safestringlib/safe_lib.h
        third_party/safestringlib/safe_types.h
        third_party/safestringlib/safe_lib_errno.h
        third_party/safestringlib/safe_str_lib.h
        third_party/safestringlib/safe_str_constraint.h
        third_party/safestringlib/safe_str_constraint.c
        third_party/safestringlib/ignore_handler_s.c
        $<$<NOT:$<BOOL:${HAVE_STRNLEN_S}>>:third_party/safestringlib/strnlen_s.c>
        $<$<NOT:$<BOOL:${HAVE_STRNCPY_S}>>:third_party/safestringlib/strncpy_s.c>
        $<$<NOT:$<BOOL:${HAVE_STRCPY_S}>>:third_party/safestringlib/strcpy_s.c>)
endif()

# Try to find valgrind.h for the RUNNING_ON_VALGRIND macro
# needed for now for svt_dav1d_inv_txfm_add_dct_dct_32x16_8bpc_avx2
# due to the later crashing libVEX when running on valgrind
# TODO: remove this once the issue is fixed
include(CheckIncludeFile)
check_include_file("valgrind/valgrind.h" HAVE_VALGRIND_H)
if(HAVE_VALGRIND_H)
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_VALGRIND_H=1)
else()
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_VALGRIND_H=0)
endif()

# try to see if __builtin_expect is available
check_c_source_compiles("
    int main() {
        if (__builtin_expect(1, 0)) {
            return 1;
        }
        return 0;
    }" HAVE_BUILTIN_EXPECT)
if(HAVE_BUILTIN_EXPECT)
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_BUILTIN_EXPECT=1)
else()
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_BUILTIN_EXPECT=0)
endif()

include_directories(.)

# Find out if we have threading available
find_package(Threads)

check_symbol_exists(elf_aux_info "sys/auxv.h" HAVE_ELF_AUX_INFO)
if(HAVE_ELF_AUX_INFO)
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_ELF_AUX_INFO=1)
else()
    set_property(DIRECTORY .
        APPEND
        PROPERTY COMPILE_DEFINITIONS
            HAVE_ELF_AUX_INFO=0)
endif()

string(TOLOWER "${SANITIZER}" SANITIZER)

# Address Memory Thread Undefined
if(SANITIZER STREQUAL "address")
    add_clike_and_ld_flags(-fsanitize=address)
elseif(SANITIZER STREQUAL "memory")
    add_clike_and_ld_flags(-fsanitize=memory -fsanitize-memory-track-origins)
elseif(SANITIZER STREQUAL "thread")
    add_clike_and_ld_flags(-fsanitize=thread)
elseif(SANITIZER STREQUAL "undefined")
    add_clike_and_ld_flags(-fsanitize=undefined)
elseif(SANITIZER STREQUAL "integer")
    add_clike_and_ld_flags(-fsanitize=integer)
elseif(SANITIZER)
    message(WARNING "Unknown sanitizer: ${SANITIZER}, going to try adding it anyway")
    add_clike_and_ld_flags(-fsanitize=${SANITIZER})
endif()

if(SANITIZER)
    check_both_flags_add(-fno-omit-frame-pointer -fno-optimize-sibling-calls)
endif()

if(COVERAGE)
    add_clike_and_ld_flags(--coverage)
endif()

set(SVT_AV1_INCLUDEDIR "${CMAKE_INSTALL_INCLUDEDIR}")
set(SVT_AV1_LIBDIR "${CMAKE_INSTALL_LIBDIR}")

if(NOT IS_ABSOLUTE "${SVT_AV1_INCLUDEDIR}")
    string(PREPEND SVT_AV1_INCLUDEDIR "\${prefix}/")
endif()
if(NOT IS_ABSOLUTE "${CMAKE_INSTALL_LIBDIR}")
    string(PREPEND SVT_AV1_LIBDIR "\${exec_prefix}/")
endif()

if(UNIX AND NOT APPLE)
    list(APPEND PLATFORM_LIBS Threads::Threads m)
    list(APPEND PC_LIBS_PRIVATE ${CMAKE_THREAD_LIBS_INIT} -lm)
endif()

# Add Subdirectories
add_subdirectory(Source/Lib)
if(BUILD_APPS)
    add_subdirectory(Source/App)
endif()
if(SVT_AV1_PGO)
    string(REPLACE ";" "::" SVT_AV1_PGO_CUSTOM_VIDEOS "${SVT_AV1_PGO_CUSTOM_VIDEOS}")

    add_custom_target(PGOCompileGen
        ${CMAKE_COMMAND} ${CMAKE_BINARY_DIR} -DSVT_INTERNAL_PGO_GENERATE=ON -DSVT_INTERNAL_PGO_USE=OFF
        COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR}
        --clean-first
        --target SvtAv1EncApp
        COMMENT "Compiling with PGO generate"
        VERBATIM
        USES_TERMINAL
    )

    if(CMAKE_C_COMPILER_ID MATCHES "Clang")
        # We want unprofiled code optimised for speed, not size, so we mark the profiling data as "sparse".
        set(PROFILE_MERGE_CMD sh -c "${LLVM_PROFDATA} merge --sparse=true *.profraw -o default.profdata")
    else()
        # Do nothing. GCC has no profile merging step.
        set(PROFILE_MERGE_CMD)
    endif()

    add_custom_target(PGOCompileUse
        ${CMAKE_COMMAND} ${CMAKE_BINARY_DIR} -DSVT_INTERNAL_PGO_GENERATE=OFF -DSVT_INTERNAL_PGO_USE=ON
        COMMAND ${PROFILE_MERGE_CMD}
        COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR}
        --clean-first
        --target SvtAv1EncApp
        COMMENT "Compiling PGO use"
        COMMAND_EXPAND_LISTS
        VERBATIM
        USES_TERMINAL
    )

    if(SVT_AV1_PGO_CUSTOM_VIDEOS)
        add_custom_target(PGOGenerateProfile ${CMAKE_COMMAND} -P
            ${PROJECT_SOURCE_DIR}/Build/pgohelper.cmake
            ${CMAKE_BINARY_DIR}
            ${SVT_AV1_PGO_CUSTOM_VIDEOS}
            $<TARGET_FILE:SvtAv1EncApp>
            COMMENT "Generating PGO profile"
            VERBATIM
            USES_TERMINAL
        )
    else()
        add_custom_target(DownloadObjOneFast ${CMAKE_COMMAND} -P
            ${PROJECT_SOURCE_DIR}/Build/obj1fastdownloader.cmake
            ${CMAKE_BINARY_DIR}
            COMMENT "Downloading video file"
            VERBATIM
            USES_TERMINAL
        )
        add_custom_target(PGOGenerateProfile ${CMAKE_COMMAND} -P
            ${PROJECT_SOURCE_DIR}/Build/pgohelper.cmake
            ${CMAKE_BINARY_DIR}
            ${CMAKE_BINARY_DIR}/objective-1-fast
            $<TARGET_FILE:SvtAv1EncApp>
            COMMENT "Generating PGO profile"
            DEPENDS DownloadObjOneFast
            VERBATIM
            USES_TERMINAL
        )
    endif()

    add_custom_target(RunPGO
        ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR} --config $<CONFIG> --target PGOCompileGen
        COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR} --config $<CONFIG> --target PGOGenerateProfile
        COMMAND ${CMAKE_COMMAND} --build ${CMAKE_BINARY_DIR} --config $<CONFIG> --target PGOCompileUse
        COMMENT "Compiling PGO Binary"
        VERBATIM
        USES_TERMINAL
    )
endif()

if(BUILD_TESTING)
    include(CTest)
    message(STATUS "Building UnitTests")
    add_subdirectory(test)
    add_subdirectory(third_party/googletest)
endif()

add_subdirectory(third_party/fastfeat)
