
#include "edge-segments.h"

#include "arithmetics.hpp"
#include "equation-solver.h"

namespace msdfgen {

void EdgeSegment::distanceToPseudoDistance(SignedDistance &distance, Point2 origin, double param) const {
    if (param < 0) {
        Vector2 dir = direction(0).normalize();
        Vector2 aq = origin-point(0);
        double ts = dotProduct(aq, dir);
        if (ts < 0) {
            double pseudoDistance = crossProduct(aq, dir);
            if (fabs(pseudoDistance) <= fabs(distance.distance)) {
                distance.distance = pseudoDistance;
                distance.dot = 0;
            }
        }
    } else if (param > 1) {
        Vector2 dir = direction(1).normalize();
        Vector2 bq = origin-point(1);
        double ts = dotProduct(bq, dir);
        if (ts > 0) {
            double pseudoDistance = crossProduct(bq, dir);
            if (fabs(pseudoDistance) <= fabs(distance.distance)) {
                distance.distance = pseudoDistance;
                distance.dot = 0;
            }
        }
    }
}

LinearSegment::LinearSegment(Point2 p0, Point2 p1, EdgeColor edgeColor) : EdgeSegment(edgeColor) {
    p[0] = p0;
    p[1] = p1;
}

QuadraticSegment::QuadraticSegment(Point2 p0, Point2 p1, Point2 p2, EdgeColor edgeColor) : EdgeSegment(edgeColor) {
    p[0] = p0;
    p[1] = p1;
    p[2] = p2;
}

CubicSegment::CubicSegment(Point2 p0, Point2 p1, Point2 p2, Point2 p3, EdgeColor edgeColor) : EdgeSegment(edgeColor) {
    p[0] = p0;
    p[1] = p1;
    p[2] = p2;
    p[3] = p3;
}

LinearSegment * LinearSegment::clone() const {
    return new LinearSegment(p[0], p[1], color);
}

QuadraticSegment * QuadraticSegment::clone() const {
    return new QuadraticSegment(p[0], p[1], p[2], color);
}

CubicSegment * CubicSegment::clone() const {
    return new CubicSegment(p[0], p[1], p[2], p[3], color);
}

Point2 LinearSegment::point(double param) const {
    return mix(p[0], p[1], param);
}

Point2 QuadraticSegment::point(double param) const {
    return mix(mix(p[0], p[1], param), mix(p[1], p[2], param), param);
}

Point2 CubicSegment::point(double param) const {
    Vector2 p12 = mix(p[1], p[2], param);
    return mix(mix(mix(p[0], p[1], param), p12, param), mix(p12, mix(p[2], p[3], param), param), param);
}

Vector2 LinearSegment::direction(double param) const {
    return p[1]-p[0];
}

Vector2 QuadraticSegment::direction(double param) const {
    return mix(p[1]-p[0], p[2]-p[1], param);
}

Vector2 CubicSegment::direction(double param) const {
    return mix(mix(p[1]-p[0], p[2]-p[1], param), mix(p[2]-p[1], p[3]-p[2], param), param);
}

SignedDistance LinearSegment::signedDistance(Point2 origin, double &param) const {
    Vector2 aq = origin-p[0];
    Vector2 ab = p[1]-p[0];
    param = dotProduct(aq, ab)/dotProduct(ab, ab);
    Vector2 eq = p[param > .5]-origin;
    double endpointDistance = eq.length();
    if (param > 0 && param < 1) {
        double orthoDistance = dotProduct(ab.getOrthonormal(false), aq);
        if (fabs(orthoDistance) < endpointDistance)
            return SignedDistance(orthoDistance, 0);
    }
    return SignedDistance(nonZeroSign(crossProduct(aq, ab))*endpointDistance, fabs(dotProduct(ab.normalize(), eq.normalize())));
}

SignedDistance QuadraticSegment::signedDistance(Point2 origin, double &param) const {
    Vector2 qa = p[0]-origin;
    Vector2 ab = p[1]-p[0];
    Vector2 br = p[0]+p[2]-p[1]-p[1];
    double a = dotProduct(br, br);
    double b = 3*dotProduct(ab, br);
    double c = 2*dotProduct(ab, ab)+dotProduct(qa, br);
    double d = dotProduct(qa, ab);
    double t[3];
    int solutions = solveCubic(t, a, b, c, d);

    double minDistance = nonZeroSign(crossProduct(ab, qa))*qa.length(); // distance from A
    param = -dotProduct(qa, ab)/dotProduct(ab, ab);
    {
        double distance = nonZeroSign(crossProduct(p[2]-p[1], p[2]-origin))*(p[2]-origin).length(); // distance from B
        if (fabs(distance) < fabs(minDistance)) {
            minDistance = distance;
            param = dotProduct(origin-p[1], p[2]-p[1])/dotProduct(p[2]-p[1], p[2]-p[1]);
        }
    }
    for (int i = 0; i < solutions; ++i) {
        if (t[i] > 0 && t[i] < 1) {
            Point2 endpoint = p[0]+2*t[i]*ab+t[i]*t[i]*br;
            double distance = nonZeroSign(crossProduct(p[2]-p[0], endpoint-origin))*(endpoint-origin).length();
            if (fabs(distance) <= fabs(minDistance)) {
                minDistance = distance;
                param = t[i];
            }
        }
    }

    if (param >= 0 && param <= 1)
        return SignedDistance(minDistance, 0);
    if (param < .5)
        return SignedDistance(minDistance, fabs(dotProduct(ab.normalize(), qa.normalize())));
    else
        return SignedDistance(minDistance, fabs(dotProduct((p[2]-p[1]).normalize(), (p[2]-origin).normalize())));
}

SignedDistance CubicSegment::signedDistance(Point2 origin, double &param) const {
    Vector2 qa = p[0]-origin;
    Vector2 ab = p[1]-p[0];
    Vector2 br = p[2]-p[1]-ab;
    Vector2 as = (p[3]-p[2])-(p[2]-p[1])-br;

    double minDistance = nonZeroSign(crossProduct(ab, qa))*qa.length(); // distance from A
    param = -dotProduct(qa, ab)/dotProduct(ab, ab);
    {
        double distance = nonZeroSign(crossProduct(p[3]-p[2], p[3]-origin))*(p[3]-origin).length(); // distance from B
        if (fabs(distance) < fabs(minDistance)) {
            minDistance = distance;
            param = dotProduct(origin-p[2], p[3]-p[2])/dotProduct(p[3]-p[2], p[3]-p[2]);
        }
    }
    // Iterative minimum distance search
    for (int i = 0; i <= MSDFGEN_CUBIC_SEARCH_STARTS; ++i) {
        double t = (double) i/MSDFGEN_CUBIC_SEARCH_STARTS;
        for (int step = 0;; ++step) {
            Vector2 qpt = point(t)-origin;
            double distance = nonZeroSign(crossProduct(direction(t), qpt))*qpt.length();
            if (fabs(distance) < fabs(minDistance)) {
                minDistance = distance;
                param = t;
            }
            if (step == MSDFGEN_CUBIC_SEARCH_STEPS)
                break;
            // Improve t
            Vector2 d1 = 3*as*t*t+6*br*t+3*ab;
            Vector2 d2 = 6*as*t+6*br;
            t -= dotProduct(qpt, d1)/(dotProduct(d1, d1)+dotProduct(qpt, d2));
            if (t < 0 || t > 1)
                break;
        }
    }

    if (param >= 0 && param <= 1)
        return SignedDistance(minDistance, 0);
    if (param < .5)
        return SignedDistance(minDistance, fabs(dotProduct(ab.normalize(), qa.normalize())));
    else
        return SignedDistance(minDistance, fabs(dotProduct((p[3]-p[2]).normalize(), (p[3]-origin).normalize())));
}

// Original method by solving a fifth order polynomial
/*SignedDistance CubicSegment::signedDistance(Point2 origin, double &param) const {
    Vector2 qa = p[0]-origin;
    Vector2 ab = p[1]-p[0];
    Vector2 br = p[2]-p[1]-ab;
    Vector2 as = (p[3]-p[2])-(p[2]-p[1])-br;
    double a = dotProduct(as, as);
    double b = 5*dotProduct(br, as);
    double c = 4*dotProduct(ab, as)+6*dotProduct(br, br);
    double d = 9*dotProduct(ab, br)+dotProduct(qa, as);
    double e = 3*dotProduct(ab, ab)+2*dotProduct(qa, br);
    double f = dotProduct(qa, ab);
    double t[5];
    int solutions = solveQuintic(t, a, b, c, d, e, f);

    double minDistance = nonZeroSign(crossProduct(ab, qa))*qa.length(); // distance from A
    param = -dotProduct(qa, ab)/dotProduct(ab, ab);
    {
        double distance = nonZeroSign(crossProduct(p[3]-p[2], p[3]-origin))*(p[3]-origin).length(); // distance from B
        if (fabs(distance) < fabs(minDistance)) {
            minDistance = distance;
            param = dotProduct(origin-p[2], p[3]-p[2])/dotProduct(p[3]-p[2], p[3]-p[2]);
        }
    }
    for (int i = 0; i < solutions; ++i) {
        if (t[i] > 0 && t[i] < 1) {
            Point2 endpoint = p[0]+3*t[i]*ab+3*t[i]*t[i]*br+t[i]*t[i]*t[i]*as;
            Vector2 dirVec = t[i]*t[i]*as+2*t[i]*br+ab;
            double distance = nonZeroSign(crossProduct(dirVec, endpoint-origin))*(endpoint-origin).length();
            if (fabs(distance) <= fabs(minDistance)) {
                minDistance = distance;
                param = t[i];
            }
        }
    }

    if (param >= 0 && param <= 1)
        return SignedDistance(minDistance, 0);
    if (param < .5)
        return SignedDistance(minDistance, fabs(dotProduct(ab.normalize(), qa.normalize())));
    else
        return SignedDistance(minDistance, fabs(dotProduct((p[3]-p[2]).normalize(), (p[3]-origin).normalize())));
}*/

static void pointBounds(Point2 p, double &l, double &b, double &r, double &t) {
    if (p.x < l) l = p.x;
    if (p.y < b) b = p.y;
    if (p.x > r) r = p.x;
    if (p.y > t) t = p.y;
}

void LinearSegment::bounds(double &l, double &b, double &r, double &t) const {
    pointBounds(p[0], l, b, r, t);
    pointBounds(p[1], l, b, r, t);
}

void QuadraticSegment::bounds(double &l, double &b, double &r, double &t) const {
    pointBounds(p[0], l, b, r, t);
    pointBounds(p[2], l, b, r, t);
    Vector2 bot = (p[1]-p[0])-(p[2]-p[1]);
    if (bot.x) {
        double param = (p[1].x-p[0].x)/bot.x;
        if (param > 0 && param < 1)
            pointBounds(point(param), l, b, r, t);
    }
    if (bot.y) {
        double param = (p[1].y-p[0].y)/bot.y;
        if (param > 0 && param < 1)
            pointBounds(point(param), l, b, r, t);
    }
}

void CubicSegment::bounds(double &l, double &b, double &r, double &t) const {
    pointBounds(p[0], l, b, r, t);
    pointBounds(p[3], l, b, r, t);
    Vector2 a0 = p[1]-p[0];
    Vector2 a1 = 2*(p[2]-p[1]-a0);
    Vector2 a2 = p[3]-3*p[2]+3*p[1]-p[0];
    double params[2];
    int solutions;
    solutions = solveQuadratic(params, a2.x, a1.x, a0.x);
    for (int i = 0; i < solutions; ++i)
        if (params[i] > 0 && params[i] < 1)
            pointBounds(point(params[i]), l, b, r, t);
    solutions = solveQuadratic(params, a2.y, a1.y, a0.y);
    for (int i = 0; i < solutions; ++i)
        if (params[i] > 0 && params[i] < 1)
            pointBounds(point(params[i]), l, b, r, t);
}

void LinearSegment::moveStartPoint(Point2 to) {
    p[0] = to;
}

void QuadraticSegment::moveStartPoint(Point2 to) {
    Vector2 origSDir = p[0]-p[1];
    Point2 origP1 = p[1];
    p[1] += crossProduct(p[0]-p[1], to-p[0])/crossProduct(p[0]-p[1], p[2]-p[1])*(p[2]-p[1]);
    p[0] = to;
    if (dotProduct(origSDir, p[0]-p[1]) < 0)
        p[1] = origP1;
}

void CubicSegment::moveStartPoint(Point2 to) {
    p[1] += to-p[0];
    p[0] = to;
}

void LinearSegment::moveEndPoint(Point2 to) {
    p[1] = to;
}

void QuadraticSegment::moveEndPoint(Point2 to) {
    Vector2 origEDir = p[2]-p[1];
    Point2 origP1 = p[1];
    p[1] += crossProduct(p[2]-p[1], to-p[2])/crossProduct(p[2]-p[1], p[0]-p[1])*(p[0]-p[1]);
    p[2] = to;
    if (dotProduct(origEDir, p[2]-p[1]) < 0)
        p[1] = origP1;
}

void CubicSegment::moveEndPoint(Point2 to) {
    p[2] += to-p[3];
    p[3] = to;
}

void LinearSegment::splitInThirds(EdgeSegment *&part1, EdgeSegment *&part2, EdgeSegment *&part3) const {
    part1 = new LinearSegment(p[0], point(1/3.), color);
    part2 = new LinearSegment(point(1/3.), point(2/3.), color);
    part3 = new LinearSegment(point(2/3.), p[1], color);
}

void QuadraticSegment::splitInThirds(EdgeSegment *&part1, EdgeSegment *&part2, EdgeSegment *&part3) const {
    part1 = new QuadraticSegment(p[0], mix(p[0], p[1], 1/3.), point(1/3.), color);
    part2 = new QuadraticSegment(point(1/3.), mix(mix(p[0], p[1], 5/9.), mix(p[1], p[2], 4/9.), .5), point(2/3.), color);
    part3 = new QuadraticSegment(point(2/3.), mix(p[1], p[2], 2/3.), p[2], color);
}

void CubicSegment::splitInThirds(EdgeSegment *&part1, EdgeSegment *&part2, EdgeSegment *&part3) const {
    part1 = new CubicSegment(p[0], mix(p[0], p[1], 1/3.), mix(mix(p[0], p[1], 1/3.), mix(p[1], p[2], 1/3.), 1/3.), point(1/3.), color);
    part2 = new CubicSegment(point(1/3.),
        mix(mix(mix(p[0], p[1], 1/3.), mix(p[1], p[2], 1/3.), 1/3.), mix(mix(p[1], p[2], 1/3.), mix(p[2], p[3], 1/3.), 1/3.), 2/3.),
        mix(mix(mix(p[0], p[1], 2/3.), mix(p[1], p[2], 2/3.), 2/3.), mix(mix(p[1], p[2], 2/3.), mix(p[2], p[3], 2/3.), 2/3.), 1/3.),
        point(2/3.), color);
    part3 = new CubicSegment(point(2/3.), mix(mix(p[1], p[2], 2/3.), mix(p[2], p[3], 2/3.), 2/3.), mix(p[2], p[3], 2/3.), p[3], color);
}

}
