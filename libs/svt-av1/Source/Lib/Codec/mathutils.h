/*
 * Copyright (c) 2017, Alliance for Open Media. All rights reserved
 *
 * This source code is subject to the terms of the BSD 2 Clause License and
 * the Alliance for Open Media Patent License 1.0. If the BSD 2 Clause License
 * was not distributed with this source code in the LICENSE file, you can
 * obtain it at https://www.aomedia.org/license/software-license. If the Alliance for Open
 * Media Patent License 1.0 was not distributed with this source code in the
 * PATENTS file, you can obtain it at https://www.aomedia.org/license/patent-license.
 */

#ifndef AOM_AV1_ENCODER_MATHUTILS_H_
#define AOM_AV1_ENCODER_MATHUTILS_H_

#include <memory.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <assert.h>

// Solves Ax = b, where x and b are column vectors of size nx1 and A is nxn
static INLINE int32_t linsolve(int32_t n, double* A, int32_t stride, double* b, double* x) {
    const double tiny_near_zero = 1.0E-16;
    int32_t      i, j, k;
    double       c;
    // Forward elimination
    for (k = 0; k < n - 1; k++) {
        // Bring the largest magnitude to the diagonal position
        for (i = n - 1; i > k; i--) {
            if (fabs(A[(i - 1) * stride + k]) < fabs(A[i * stride + k])) {
                for (j = 0; j < n; j++) {
                    c                       = A[i * stride + j];
                    A[i * stride + j]       = A[(i - 1) * stride + j];
                    A[(i - 1) * stride + j] = c;
                }
                c        = b[i];
                b[i]     = b[i - 1];
                b[i - 1] = c;
            }
        }
        for (i = k; i < n - 1; i++) {
            if (fabs(A[k * stride + k]) < tiny_near_zero) {
                return 0;
            }
            c = A[(i + 1) * stride + k] / A[k * stride + k];
            for (j = 0; j < n; j++) {
                A[(i + 1) * stride + j] -= c * A[k * stride + j];
            }
            b[i + 1] -= c * b[k];
        }
    }
    // Backward substitution
    for (i = n - 1; i >= 0; i--) {
        if (fabs(A[i * stride + i]) < tiny_near_zero) {
            return 0;
        }
        c = 0;
        for (j = i + 1; j <= n - 1; j++) {
            c += A[i * stride + j] * x[j];
        }
        x[i] = (b[i] - c) / A[i * stride + i];
    }

    return 1;
}

// Least-squares
// Solves for n-dim x in a least squares sense to minimize |Ax - b|^2
// The solution is simply x = (A'A)^-1 A'b or simply the solution for
// the system: A'A x = A'b
//
// This process is split into three steps in order to avoid needing to
// explicitly allocate the A matrix, which may be very large if there
// are many equations to solve.
//
// The process for using this is (in pseudocode):
//
// Allocate mat (size n*n), y (size n), a (size n), x (size n)
// least_squares_init(mat, y, n)
// for each equation a . x = b {
//    least_squares_accumulate(mat, y, a, b, n)
// }
// least_squares_solve(mat, y, x, n)
//
// where:
// * mat, y are accumulators for the values A'A and A'b respectively,
// * a, b are the coefficients of each individual equation,
// * x is the result vector
// * and n is the problem size
static INLINE void least_squares_init(double* mat, double* y, int n) {
    memset(mat, 0, n * n * sizeof(double));
    memset(y, 0, n * sizeof(double));
}

static INLINE void least_squares_accumulate(double* mat, double* y, const double* a, double b, int n) {
    for (int i = 0; i < n; i++) {
        for (int j = 0; j < n; j++) {
            mat[i * n + j] += a[i] * a[j];
        }
    }
    for (int i = 0; i < n; i++) {
        y[i] += a[i] * b;
    }
}

static INLINE int least_squares_solve(double* mat, double* y, double* x, int n) {
    return linsolve(n, mat, n, y, x);
}

// Matrix multiply
static INLINE void multiply_mat(const double* m1, const double* m2, double* res, const int32_t m1_rows,
                                const int32_t inner_dim, const int32_t m2_cols) {
    double sum;

    int32_t row, col, inner;
    for (row = 0; row < m1_rows; ++row) {
        for (col = 0; col < m2_cols; ++col) {
            sum = 0;
            for (inner = 0; inner < inner_dim; ++inner) {
                sum += m1[row * inner_dim + inner] * m2[inner * m2_cols + col];
            }
            *(res++) = sum;
        }
    }
}

#endif // AOM_AV1_ENCODER_MATHUTILS_H_
