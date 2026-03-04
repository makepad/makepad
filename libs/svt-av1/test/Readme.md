# Unit Tests for SVT-AV1 Encoder

## Table of Contents

1. [Introduction](#Introduction)
2. [Build and Run the Tests](#Build-the-tests)
3. [Test Results Summary](#Test-Results)
4. [FAQ](#FAQ)

## Introduction

This document gives a brief introduction on how to use the unit tests for SVT-AV1 encoder. The unit tests are developed based on gtest framework. The test suites will be published in 4 stages, the following is covered in Stage 1:

1. Unit tests for the modules, including: transform, quant, entropy coding.
2. API tests: verify the APIs by running with different parameters.
3. End-to-End tests: verify the encoder by checking reconstructed frame and decoded frame.

The development of SVT-AV1 unit testing is in working progress, more test suites will be implemented and released in the later stages, and this document will be kept updating.

## Build and Run Tests

### MacOS or Linux (64-bit)

Build the test program by running the build.sh with "test" as the second parameter:

``` bash
./build.sh release test
```

The script will download test data automatically.

Binaries can be found under `Bin/Release` and/or `Bin/Debug` named `SvtAv1UnitTests`, `SvtAv1ApiTests` and `SvtAv1E2ETests`.

Before running the test program, please register `SVT_AV1_TEST_VECTOR_PATH` in your `PATH` if you already downloaded test vector manually. The tests require a bash shell and can be run in the following manners:

``` bash
# run all the tests in Unit tests
./SvtAv1UnitTests
# list the available test cases:
./SvtAv1UnitTests --gtest_list_tests
# or run the specific test cases, for example the test cases whose name contain "transform"
./SvtAv1UnitTests --gtest_filter="*transform*"
```

### Windows(64-bit)

Generate the Visual Studio* 2017 project files by following the steps below

1. `cd Build\windows`
2. run generate_vs17.bat [such would generate the visual studio project files]
3. Open the "svt-av1.sln" using Visual Studio* 2017
4. click on Build -- > Build Solution

Test program binaries can be found under `Bin/Release` and/or `Bin/Debug` named `SvtAv1UnitTests`, `SvtAv1ApiTests` and `SvtAv1E2ETests`.

Before running the test program, please build `TestVectors` project to download the test vectors, or register `SVT_AV1_TEST_VECTOR_PATH` in your `PATH` if you already downloaded test vector manually.

The individual test program can be run in command terminal in the following manners:

``` batch
rem run all the tests in Unit tests
SvtAv1UnitTests
rem list the available test cases:
SvtAv1UnitTests --gtest_list_tests
rem or run the specific test cases, for example the test cases whose name contain "transform"
SvtAv1UnitTests --gtest_filter="*transform*"
```

## Test Results Summary

Here is the test results summary on commit: [3009e99](https://github.com/AOMediaCodec/SVT-AV1/commit/3009e99f32e3476e028aadd17a265630f80a8e36). The developers can use this summary as a reference.

Unit Tests on Ubuntu 16.04:

``` none
[==========] 169 tests from 10 test cases ran. (116106 ms total)
[  PASSED  ] 163 tests.
[  FAILED  ] 6 tests, listed below:
[  FAILED  ] Quant/QuantizeTest.input_dcac_minmax_q_n/0, where GetParam() = (2, 8)
[  FAILED  ] Quant/QuantizeTest.input_dcac_minmax_q_n/2, where GetParam() = (3, 8)
[  FAILED  ] Quant/QuantizeTest.input_dcac_minmax_q_n/4, where GetParam() = (4, 8)
[  FAILED  ] Quant/QuantizeTest.input_random_all_q_all/0, where GetParam() = (2, 8)
[  FAILED  ] Quant/QuantizeTest.input_random_all_q_all/2, where GetParam() = (3, 8)
[  FAILED  ] Quant/QuantizeTest.input_random_all_q_all/4, where GetParam() = (4, 8)
```

API Tests on Ubuntu 16.04:

``` none
[==========] 51 tests from 51 test cases ran. (30285 ms total)
[  PASSED  ] 29 tests.
[  FAILED  ] 22 tests, listed below:
[  FAILED  ] EncParamIntraPeridLenTest.run_paramter_check
[  FAILED  ] EncParamHierarchicalLvlTest.run_paramter_check
[  FAILED  ] EncParamFrameRateTest.run_paramter_check
[  FAILED  ] EncParamCompr10BitFmtTest.run_paramter_check
[  FAILED  ] EncParamSbSizeTest.run_paramter_check
[  FAILED  ] EncParamSuperBlockSizeTest.run_paramter_check
[  FAILED  ] EncParamPartitionDepthTest.run_paramter_check
[  FAILED  ] EncParamQPTest.run_paramter_check
[  FAILED  ] EncParamEnableWarpedMotionTest.run_paramter_check
[  FAILED  ] EncParamExtBlockTest.run_paramter_check
[  FAILED  ] EncParamRateCtrlModeTest.run_paramter_check
[  FAILED  ] EncParamSceneChangeDectTest.run_paramter_check
[  FAILED  ] EncParamLookAheadDistanceTest.run_paramter_check
[  FAILED  ] EncParamMaxQPAllowTest.run_paramter_check
[  FAILED  ] EncParamMinQPAllowTest.run_paramter_check
[  FAILED  ] EncParamProfileTest.run_paramter_check
[  FAILED  ] EncParamTierTest.run_paramter_check
[  FAILED  ] EncParamAsmTypeTest.run_paramter_check
[  FAILED  ] EncParamActiveChCountTest.run_paramter_check
[  FAILED  ] EncParamInjectorFrameRateTest.run_paramter_check
[  FAILED  ] EncParamReconEnabledTest.run_paramter_check
[  FAILED  ] EncApiTest.check_null_pointer
22 FAILED TESTS
YOU HAVE 2 DISABLED TESTS
```

End-to-End Tests on Ubuntu 16.04:

``` none
[==========] 6 tests from 4 test cases ran. (81772 ms total)
[  PASSED  ] 6 tests.
```

## FAQ

1. All the End-to-End test cases fail, is that correct?\
  The test vectors should be downloaded before running the End-to-End test. If the `SVT_AV1_TEST_VECTOR_PATH` is set, the test vectors will be downloaded in the folder.\
  Otherwise, they will be downloaded into `SVT-AV1/test/vectors`. In this case, you should run the End-to-End test under `Bin/Release` or `Bin/Debug` directory.
