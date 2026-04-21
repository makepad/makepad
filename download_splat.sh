#!/bin/bash
# Download sample Gaussian Splat files in PLY and SOG formats

set -e

mkdir -p local

echo "Downloading PLY gaussian splat (biker.ply)..."
curl -L -o local/biker.ply \
  "https://raw.githubusercontent.com/willeastcott/assets/main/biker.ply"

echo "Downloading SOG gaussian splat (toy-cat.sog)..."
curl -L -o local/toy-cat.sog \
  "https://developer.playcanvas.com/assets/toy-cat.sog"

echo "Done."
ls -lh local/biker.ply local/toy-cat.sog
