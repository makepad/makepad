
#include "equation-solver.h"

#define _USE_MATH_DEFINES
#include <cmath>

namespace msdfgen {

int solveQuadratic(double x[2], double a, double b, double c) {
    if (fabs(a) < 1e-14) {
        if (fabs(b) < 1e-14) {
            if (c == 0)
                return -1;
            return 0;
        }
        x[0] = -c/b;
        return 1;
    }
    double dscr = b*b-4*a*c;
    if (dscr > 0) {
        dscr = sqrt(dscr);
        x[0] = (-b+dscr)/(2*a);
        x[1] = (-b-dscr)/(2*a);
        return 2;
    } else if (dscr == 0) {
        x[0] = -b/(2*a);
        return 1;
    } else
        return 0;
}

int solveCubicNormed(double *x, double a, double b, double c) {
    double a2 = a*a;
    double q  = (a2 - 3*b)/9; 
    double r  = (a*(2*a2-9*b) + 27*c)/54;
    double r2 = r*r;
    double q3 = q*q*q;
    double A, B;
    if (r2 < q3) {
        double t = r/sqrt(q3);
        if (t < -1) t = -1;
        if (t > 1) t = 1;
        t = acos(t);
        a /= 3; q = -2*sqrt(q);
        x[0] = q*cos(t/3)-a;
        x[1] = q*cos((t+2*M_PI)/3)-a;
        x[2] = q*cos((t-2*M_PI)/3)-a;
        return 3;
    } else {
        A = -pow(fabs(r)+sqrt(r2-q3), 1/3.); 
        if (r < 0) A = -A;
        B = A == 0 ? 0 : q/A;
        a /= 3;
        x[0] = (A+B)-a;
        x[1] = -0.5*(A+B)-a;
        x[2] = 0.5*sqrt(3.)*(A-B);
        if (fabs(x[2]) < 1e-14)
            return 2;
        return 1;
    }
}

int solveCubic(double x[3], double a, double b, double c, double d) {
    if (fabs(a) < 1e-14)
        return solveQuadratic(x, b, c, d);
    return solveCubicNormed(x, b/a, c/a, d/a);
}

}
