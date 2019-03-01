
#include "Contour.h"

namespace msdfgen {

void Contour::addEdge(const EdgeHolder &edge) {
    edges.push_back(edge);
}

#ifdef MSDFGEN_USE_CPP11
void Contour::addEdge(EdgeHolder &&edge) {
    edges.push_back((EdgeHolder &&) edge);
}
#endif

EdgeHolder & Contour::addEdge() {
    edges.resize(edges.size()+1);
    return edges[edges.size()-1];
}

void Contour::bounds(double &l, double &b, double &r, double &t) const {
    for (std::vector<EdgeHolder>::const_iterator edge = edges.begin(); edge != edges.end(); ++edge)
        (*edge)->bounds(l, b, r, t);
}

}
