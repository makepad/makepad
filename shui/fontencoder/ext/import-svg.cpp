/*
#include "import-svg.h"

#include <cstdio>
#include <tinyxml2.h>

#ifdef _WIN32
    #pragma warning(disable:4996)
#endif

namespace msdfgen {

#define REQUIRE(cond) { if (!(cond)) return false; }

static bool readNodeType(char &output, const char *&pathDef) {
    int shift;
    char nodeType;
    if (sscanf(pathDef, " %c%n", &nodeType, &shift) == 1 && nodeType != '+' && nodeType != '-' && nodeType != '.' && nodeType != ',' && (nodeType < '0' || nodeType > '9')) {
        pathDef += shift;
        output = nodeType;
        return true;
    }
    return false;
}

static bool readCoord(Point2 &output, const char *&pathDef) {
    int shift;
    double x, y;
    if (sscanf(pathDef, "%lf%lf%n", &x, &y, &shift) == 2) {
        output.x = x;
        output.y = y;
        pathDef += shift;
        return true;
    }
    if (sscanf(pathDef, "%lf,%lf%n", &x, &y, &shift) == 2) {
        output.x = x;
        output.y = y;
        pathDef += shift;
        return true;
    }
    return false;
}

static bool readDouble(double &output, const char *&pathDef) {
    int shift;
    double v;
    if (sscanf(pathDef, "%lf%n", &v, &shift) == 1) {
        pathDef += shift;
        output = v;
        return true;
    }
    return false;
}

static bool buildFromPath(Shape &shape, const char *pathDef) {
    char nodeType;
    Point2 prevNode(0, 0);
    while (readNodeType(nodeType, pathDef)) {
        Contour &contour = shape.addContour();
        bool contourStart = true;

        Point2 startPoint;
        Point2 controlPoint[2];
        Point2 node;

        while (true) {
            switch (nodeType) {
                case 'M':
                    REQUIRE(contourStart);
                    REQUIRE(readCoord(node, pathDef));
                    startPoint = node;
                    nodeType = 'L';
                    break;
                case 'm':
                    REQUIRE(contourStart);
                    REQUIRE(readCoord(node, pathDef));
                    node += prevNode;
                    startPoint = node;
                    nodeType = 'l';
                    break;
                case 'Z':
                case 'z':
                    if (prevNode != startPoint)
                        contour.addEdge(new LinearSegment(prevNode, startPoint));
                    goto NEXT_CONTOUR;
                case 'L':
                    REQUIRE(readCoord(node, pathDef));
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'l':
                    REQUIRE(readCoord(node, pathDef));
                    node += prevNode;
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'H':
                    REQUIRE(readDouble(node.x, pathDef));
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'h':
                    REQUIRE(readDouble(node.x, pathDef));
                    node.x += prevNode.x;
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'V':
                    REQUIRE(readDouble(node.y, pathDef));
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'v':
                    REQUIRE(readDouble(node.y, pathDef));
                    node.y += prevNode.y;
                    contour.addEdge(new LinearSegment(prevNode, node));
                    break;
                case 'Q':
                    REQUIRE(readCoord(controlPoint[0], pathDef));
                    REQUIRE(readCoord(node, pathDef));
                    contour.addEdge(new QuadraticSegment(prevNode, controlPoint[0], node));
                    break;
                case 'q':
                    REQUIRE(readCoord(controlPoint[0], pathDef));
                    REQUIRE(readCoord(node, pathDef));
                    controlPoint[0] += prevNode;
                    node += prevNode;
                    contour.addEdge(new QuadraticSegment(prevNode, controlPoint[0], node));
                    break;
                // TODO T, t
                case 'C':
                    REQUIRE(readCoord(controlPoint[0], pathDef));
                    REQUIRE(readCoord(controlPoint[1], pathDef));
                    REQUIRE(readCoord(node, pathDef));
                    contour.addEdge(new CubicSegment(prevNode, controlPoint[0], controlPoint[1], node));
                    break;
                case 'c':
                    REQUIRE(readCoord(controlPoint[0], pathDef));
                    REQUIRE(readCoord(controlPoint[1], pathDef));
                    REQUIRE(readCoord(node, pathDef));
                    controlPoint[0] += prevNode;
                    controlPoint[1] += prevNode;
                    node += prevNode;
                    contour.addEdge(new CubicSegment(prevNode, controlPoint[0], controlPoint[1], node));
                    break;
                // TODO S, s
                // TODO A, a
                default:
                    REQUIRE(false);
            }
            contourStart &= nodeType == 'M' || nodeType == 'm';
            prevNode = node;
            readNodeType(nodeType, pathDef);
        }
    NEXT_CONTOUR:;
    }
    return true;
}

bool loadSvgShape(Shape &output, const char *filename, Vector2 *dimensions) {
    tinyxml2::XMLDocument doc;
    if (doc.LoadFile(filename))
        return false;
    tinyxml2::XMLElement *root = doc.FirstChildElement("svg");
    if (!root)
        return false;

    tinyxml2::XMLElement *path = root->FirstChildElement("path");
    if (!path) {
        tinyxml2::XMLElement *g = root->FirstChildElement("g");
        if (g)
            path = g->FirstChildElement("path");
    }
    if (!path)
        return false;
    const char *pd = path->Attribute("d");
    if (!pd)
        return false;

    output.contours.clear();
    output.inverseYAxis = true;
    if (dimensions) {
        dimensions->x = root->DoubleAttribute("width");
        dimensions->y = root->DoubleAttribute("height");
    }
    return buildFromPath(output, pd);
}

}*/
