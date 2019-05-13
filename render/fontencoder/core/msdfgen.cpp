
#include "../msdfgen.h"

#include "arithmetics.hpp"

namespace msdfgen {

void generateSDF(Bitmap<float> &output, const Shape &shape, double range, const Vector2 &scale, const Vector2 &translate) {
    int w = output.width(), h = output.height();
#ifdef MSDFGEN_USE_OPENMP
    #pragma omp parallel for
#endif
    for (int y = 0; y < h; ++y) {
        int row = shape.inverseYAxis ? h-y-1 : y;
        for (int x = 0; x < w; ++x) {
            double dummy;
            Point2 p = Vector2(x+.5, y+.5)/scale-translate;
            SignedDistance minDistance;
            for (std::vector<Contour>::const_iterator contour = shape.contours.begin(); contour != shape.contours.end(); ++contour)
                for (std::vector<EdgeHolder>::const_iterator edge = contour->edges.begin(); edge != contour->edges.end(); ++edge) {
                    SignedDistance distance = (*edge)->signedDistance(p, dummy);
                    if (distance < minDistance)
                        minDistance = distance;
                }
            output(x, row) = float(minDistance.distance/range+.5);
        }
    }
}

void generatePseudoSDF(Bitmap<float> &output, const Shape &shape, double range, const Vector2 &scale, const Vector2 &translate) {
    int w = output.width(), h = output.height();
#ifdef MSDFGEN_USE_OPENMP
    #pragma omp parallel for
#endif
    for (int y = 0; y < h; ++y) {
        int row = shape.inverseYAxis ? h-y-1 : y;
        for (int x = 0; x < w; ++x) {
            Point2 p = Vector2(x+.5, y+.5)/scale-translate;
            SignedDistance minDistance;
            const EdgeHolder *nearEdge = NULL;
            double nearParam = 0;
            for (std::vector<Contour>::const_iterator contour = shape.contours.begin(); contour != shape.contours.end(); ++contour)
                for (std::vector<EdgeHolder>::const_iterator edge = contour->edges.begin(); edge != contour->edges.end(); ++edge) {
                    double param;
                    SignedDistance distance = (*edge)->signedDistance(p, param);
                    if (distance < minDistance) {
                        minDistance = distance;
                        nearEdge = &*edge;
                        nearParam = param;
                    }
                }
            if (nearEdge)
                (*nearEdge)->distanceToPseudoDistance(minDistance, p, nearParam);
            output(x, row) = float(minDistance.distance/range+.5);
        }
    }
}

static inline bool pixelClash(const FloatRGB &a, const FloatRGB &b, double threshold) {
    // Only consider pair where both are on the inside or both are on the outside
    bool aIn = (a.r > .5f)+(a.g > .5f)+(a.b > .5f) >= 2;
    bool bIn = (b.r > .5f)+(b.g > .5f)+(b.b > .5f) >= 2;
    if (aIn != bIn) return false;
    // If the change is 0 <-> 1 or 2 <-> 3 channels and not 1 <-> 1 or 2 <-> 2, it is not a clash
    if ((a.r > .5f && a.g > .5f && a.b > .5f) || (a.r < .5f && a.g < .5f && a.b < .5f)
        || (b.r > .5f && b.g > .5f && b.b > .5f) || (b.r < .5f && b.g < .5f && b.b < .5f))
        return false;
    // Find which color is which: _a, _b = the changing channels, _c = the remaining one
    float aa, ab, ba, bb, ac, bc;
    if ((a.r > .5f) != (b.r > .5f) && (a.r < .5f) != (b.r < .5f)) {
        aa = a.r, ba = b.r;
        if ((a.g > .5f) != (b.g > .5f) && (a.g < .5f) != (b.g < .5f)) {
            ab = a.g, bb = b.g;
            ac = a.b, bc = b.b;
        } else if ((a.b > .5f) != (b.b > .5f) && (a.b < .5f) != (b.b < .5f)) {
            ab = a.b, bb = b.b;
            ac = a.g, bc = b.g;
        } else
            return false; // this should never happen
    } else if ((a.g > .5f) != (b.g > .5f) && (a.g < .5f) != (b.g < .5f)
        && (a.b > .5f) != (b.b > .5f) && (a.b < .5f) != (b.b < .5f)) {
        aa = a.g, ba = b.g;
        ab = a.b, bb = b.b;
        ac = a.r, bc = b.r;
    } else
        return false;
    // Find if the channels are in fact discontinuous
    return (fabsf(aa-ba) >= threshold)
        && (fabsf(ab-bb) >= threshold)
        && fabsf(ac-.5f) >= fabsf(bc-.5f); // Out of the pair, only flag the pixel farther from a shape edge
}

void msdfErrorCorrection(Bitmap<FloatRGB> &output, const Vector2 &threshold) {
    std::vector<std::pair<int, int> > clashes;
    int w = output.width(), h = output.height();
    for (int y = 0; y < h; ++y)
        for (int x = 0; x < w; ++x) {
            if ((x > 0 && pixelClash(output(x, y), output(x-1, y), threshold.x))
                || (x < w-1 && pixelClash(output(x, y), output(x+1, y), threshold.x))
                || (y > 0 && pixelClash(output(x, y), output(x, y-1), threshold.y))
                || (y < h-1 && pixelClash(output(x, y), output(x, y+1), threshold.y)))
                clashes.push_back(std::make_pair(x, y));
        }
    for (std::vector<std::pair<int, int> >::const_iterator clash = clashes.begin(); clash != clashes.end(); ++clash) {
        FloatRGB &pixel = output(clash->first, clash->second);
        float med = median(pixel.r, pixel.g, pixel.b);
        pixel.r = med, pixel.g = med, pixel.b = med;
    }
}

void generateMSDF(Bitmap<FloatRGB> &output, const Shape &shape, double range, const Vector2 &scale, const Vector2 &translate, double edgeThreshold) {
    int w = output.width(), h = output.height();
#ifdef MSDFGEN_USE_OPENMP
    #pragma omp parallel for
#endif
    for (int y = 0; y < h; ++y) {
        int row = shape.inverseYAxis ? h-y-1 : y;
        for (int x = 0; x < w; ++x) {
            Point2 p = Vector2(x+.5, y+.5)/scale-translate;

            struct {
                SignedDistance minDistance;
                const EdgeHolder *nearEdge;
                double nearParam;
            } r, g, b;
            r.nearEdge = g.nearEdge = b.nearEdge = NULL;
            r.nearParam = g.nearParam = b.nearParam = 0;

            for (std::vector<Contour>::const_iterator contour = shape.contours.begin(); contour != shape.contours.end(); ++contour)
                for (std::vector<EdgeHolder>::const_iterator edge = contour->edges.begin(); edge != contour->edges.end(); ++edge) {
                    double param;
                    SignedDistance distance = (*edge)->signedDistance(p, param);
                    if ((*edge)->color&RED && distance < r.minDistance) {
                        r.minDistance = distance;
                        r.nearEdge = &*edge;
                        r.nearParam = param;
                    }
                    if ((*edge)->color&GREEN && distance < g.minDistance) {
                        g.minDistance = distance;
                        g.nearEdge = &*edge;
                        g.nearParam = param;
                    }
                    if ((*edge)->color&BLUE && distance < b.minDistance) {
                        b.minDistance = distance;
                        b.nearEdge = &*edge;
                        b.nearParam = param;
                    }
                }

            if (r.nearEdge)
                (*r.nearEdge)->distanceToPseudoDistance(r.minDistance, p, r.nearParam);
            if (g.nearEdge)
                (*g.nearEdge)->distanceToPseudoDistance(g.minDistance, p, g.nearParam);
            if (b.nearEdge)
                (*b.nearEdge)->distanceToPseudoDistance(b.minDistance, p, b.nearParam);
            output(x, row).r = float(r.minDistance.distance/range+.5);
            output(x, row).g = float(g.minDistance.distance/range+.5);
            output(x, row).b = float(b.minDistance.distance/range+.5);
        }
    }

    if (edgeThreshold > 0)
        msdfErrorCorrection(output, edgeThreshold/(scale*range));
}

}
