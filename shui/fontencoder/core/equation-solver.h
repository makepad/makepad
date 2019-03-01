
#pragma once

namespace msdfgen {

// ax^2 + bx + c = 0
int solveQuadratic(double x[2], double a, double b, double c);

// ax^3 + bx^2 + cx + d = 0
int solveCubic(double x[3], double a, double b, double c, double d);

// ax^5 + bx^4 + cx^3 + dx^2 + ex + f = 0
//int solveQuintic(double x[5], double a, double b, double c, double d, double e, double f);

}
