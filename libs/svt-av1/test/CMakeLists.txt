#
# Copyright(c) 2019 Netflix, Inc.
#
# This source code is subject to the terms of the BSD 2 Clause License and the
# Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License was
# not distributed with this source code in the LICENSE file, you can obtain it
# at https://www.aomedia.org/license/software-license. If the Alliance for Open
# Media Patent License 1.0 was not distributed with this source code in the
# PATENTS file, you can obtain it at
# https://www.aomedia.org/license/patent-license.
#

# APP Directory CMakeLists.txt

# Include Subdirectories
include_directories(
  ${PROJECT_SOURCE_DIR}/test/
  ${PROJECT_SOURCE_DIR}/test/ref
  ${PROJECT_SOURCE_DIR}/third_party/googletest/include
  third_party/googletest/src
  ${PROJECT_SOURCE_DIR}/Source/API
  ${PROJECT_SOURCE_DIR}/Source/Lib/C_DEFAULT/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE2/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSSE3/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SSE4_1/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX2/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_AVX512/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_NEON/
  ${PROJECT_SOURCE_DIR}/Source/Lib/ASM_SVE/
  ${PROJECT_SOURCE_DIR}/Source/Lib/Codec
  ${PROJECT_SOURCE_DIR}/Source/Lib/Globals
  ${PROJECT_SOURCE_DIR}/Source/App
  ${PROJECT_SOURCE_DIR}/Source/API
  ${PROJECT_SOURCE_DIR}/third_party/fastfeat/
  ${PROJECT_SOURCE_DIR}/third_party/aom_dsp/inc/)

# Define helper functions and macros used by Google Test.
include(../third_party/googletest/cmake/internal_utils.cmake)

if(MSVC)
  set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -D_ALLOW_KEYWORD_MACROS")
  set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -D_ALLOW_KEYWORD_MACROS")
endif()

# When other libraries are using a shared version of runtime libraries, Google
# Test also has to use one.
option(
  gtest_force_shared_crt
  "Use shared (DLL) run-time lib even when Google Test is built as static lib."
  OFF)

add_definitions(-D_SILENCE_TR1_NAMESPACE_DEPRECATION_WARNING=1)

enable_testing()

if(UNIX AND HAVE_X86_PLATFORM)
  set(CMAKE_C_FLAGS "${CMAKE_C_FLAGS} -mavx2")
  set(CMAKE_CXX_FLAGS "${CMAKE_CXX_FLAGS} -mavx2")
endif()

set(arch_neutral_files
    BitstreamWriterTest.cc
    unit_test.h
    unit_test_utility.c
    unit_test_utility.h
    FilmGrainExpectedResult.h
    FilmGrainTest.cc
    FwdTxfm2dApproxTest.cc
    GlobalMotionUtilTest.cc
    IntraBcUtilTest.cc
    ResizeTest.cc
    TestEnv.c
    TxfmCommon.h
    acm_random.h
    random.h
    util.h
    ref/TxfmRef.cc
    ref/TxfmRef.h
    ssim_test.cc
    svt_av1_test.cc
    ../third_party/aom_dsp/src/bitreader.c
    ../third_party/aom_dsp/src/entdec.c)

set(multi_arch_files
    AdaptiveScanTest.cc
    BlockErrorTest.cc
    CdefTest.cc
    CompoundUtilTest.cc
    Convolve8Test.cc
    DeblockTest.cc
    EncodeTxbAsmTest.cc
    FilterIntraPredTest.cc
    FwdTxfm2dAsmTest.cc
    HbdVarianceTest.cc
    InvTxfm2dAsmTest.cc
    MemTest.cc
    OBMCSadTest.cc
    OBMCVarianceTest.cc
    PackUnPackTest.cc
    PaletteModeUtilTest.cc
    PictureOperatorTest.cc
    QuantAsmTest.cc
    ResidualTest.cc
    RestorationPickTest.cc
    SadTest.cc
    SatdTest.cc
    SelfGuidedUtilTest.cc
    SpatialFullDistortionTest.cc
    TemporalFilterTestPlanewise.cc
    VarianceTest.cc
    WedgeUtilTest.cc
    av1_convolve_scale_test.cc
    compute_mean_test.cc
    convolve_test.cc
    corner_match_test.cc
    hadamard_test.cc
    intrapred_cfl_test.cc
    intrapred_dr_test.cc
    intrapred_edge_filter_test.cc
    intrapred_test.cc
    quantize_func_test.cc
    selfguided_filter_test.cc
    subtract_avg_cfl_test.cc
    warp_filter_test.cc
    warp_filter_test_util.cc
    warp_filter_test_util.h
    wiener_convolve_test.cc)

if(HAVE_X86_PLATFORM)
  set(x86_arch_files
      noise_model_test.cc
      highbd_intra_prediction_tests.cc
      highbd_intra_prediction_tests.h
      FFTTest.cc
      ForwardtransformTests.cc
      FwdTxfm1dTest.cc
      InvTxfm1dTest.cc
      FwdTxfm2dTest.cc
      MotionEstimationTest.cc
      PsnrTest.cc
      frame_error_test.cc)
endif()

if(HAVE_ARM_PLATFORM)
  set(arm_arch_files)
endif()

set(arch_neutral_lib_list
    $<TARGET_OBJECTS:FASTFEAT>
    $<TARGET_OBJECTS:C_DEFAULT>
    $<TARGET_OBJECTS:GLOBALS>
    $<TARGET_OBJECTS:CODEC>
    gtest_all)

if(HAVE_X86_PLATFORM)
  set(x86_arch_lib_list
      $<TARGET_OBJECTS:ASM_SSE2>
      $<TARGET_OBJECTS:ASM_SSSE3>
      $<TARGET_OBJECTS:ASM_SSE4_1>
      $<TARGET_OBJECTS:ASM_AVX2>)
  if(ENABLE_AVX512)
    list(APPEND x86_arch_lib_list
      $<TARGET_OBJECTS:ASM_AVX512>)
  endif()
endif()

if(HAVE_ARM_PLATFORM)
  set(arm_arch_lib_list $<TARGET_OBJECTS:ASM_NEON>)
  if(ENABLE_NEON_DOTPROD)
    list(APPEND arm_arch_lib_list $<TARGET_OBJECTS:ASM_NEON_DOTPROD>)
  endif()
  if(ENABLE_NEON_I8MM)
    list(APPEND arm_arch_lib_list $<TARGET_OBJECTS:ASM_NEON_I8MM>)
  endif()
  if(ENABLE_SVE)
    list(APPEND arm_arch_lib_list $<TARGET_OBJECTS:ASM_SVE>)
  endif()
  if(ENABLE_SVE2)
    list(APPEND arm_arch_lib_list $<TARGET_OBJECTS:ASM_SVE2>)
  endif()
endif()

set(all_files ${arch_neutral_files} ${multi_arch_files} ${x86_arch_files}
              ${arm_arch_files})

set(lib_list ${arch_neutral_lib_list} ${x86_arch_lib_list} ${arm_arch_lib_list})

if(UNIX)
  # App Source Files
  add_executable(SvtAv1UnitTests ${all_files})
  # Link the Encoder App
  target_link_libraries(SvtAv1UnitTests ${lib_list} pthread m)
else()
  cxx_executable_with_flags(SvtAv1UnitTests "${cxx_default}" "${lib_list}"
                            ${all_files})
endif()

add_dependencies(SvtAv1UnitTests SvtAv1Enc)

install(TARGETS SvtAv1UnitTests RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

add_test(NAME SvtAv1UnitTests COMMAND SvtAv1UnitTests)

add_subdirectory(api_test)
add_subdirectory(e2e_test)
