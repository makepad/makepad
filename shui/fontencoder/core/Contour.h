
#pragma once

#include <vector>
#include "EdgeHolder.h"

namespace msdfgen {

/// A single closed contour of a shape.
class Contour {

public:
    /// The sequence of edges that make up the contour.
    std::vector<EdgeHolder> edges;

    /// Adds an edge to the contour.
    void addEdge(const EdgeHolder &edge);
#ifdef MSDFGEN_USE_CPP11
    void addEdge(EdgeHolder &&edge);
#endif
    /// Creates a new edge in the contour and returns its reference.
    EdgeHolder & addEdge();
    /// Computes the bounding box of the contour.
    void bounds(double &l, double &b, double &r, double &t) const;

};

}
