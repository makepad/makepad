
#pragma once

#include <vector>
#include "Contour.h"

namespace msdfgen {

/// Vector shape representation.
class Shape {

public:
    /// The list of contours the shape consists of.
    std::vector<Contour> contours;
    /// Specifies whether the shape uses bottom-to-top (false) or top-to-bottom (true) Y coordinates.
    bool inverseYAxis;

    Shape();
    /// Adds a contour.
    void addContour(const Contour &contour);
#ifdef MSDFGEN_USE_CPP11
    void addContour(Contour &&contour);
#endif
    /// Adds a blank contour and returns its reference.
    Contour & addContour();
    /// Normalizes the shape geometry for distance field generation.
    void normalize();
    /// Performs basic checks to determine if the object represents a valid shape.
    bool validate() const;
    /// Computes the shape's bounding box.
    void bounds(double &l, double &b, double &r, double &t) const;

};

}
