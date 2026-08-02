#!/bin/sh
# Build (and optionally push) the base workspace image vm-base:x.y.z (DEC-17).
# Run on the box or any docker host — SEAL workspaces have no docker: when a
# session bumps the agent, the OPERATOR runs this and pushes.
#
#   images/build-vm-base.sh              # build vm-base:<version> locally
#   VM_BASE_REGISTRY=ghcr.io/ivan-saorin images/build-vm-base.sh   # + push
#
# Version comes from vm-base/Cargo.toml — single source of truth for the tag
# that template/Dockerfile and the mgr.toml pin refer to.
set -eu

cd "$(dirname "$0")/.."
VER=$(grep -m1 '^version' vm-base/Cargo.toml | cut -d'"' -f2)
[ -n "$VER" ] || { echo "cannot read version from vm-base/Cargo.toml"; exit 1; }
TAG="vm-base:$VER"
SHA=$(git rev-parse --short=12 HEAD 2>/dev/null || echo worktree)

echo "== building $TAG (repo @ $SHA)"
docker build -t "$TAG" .

if [ -n "${VM_BASE_REGISTRY:-}" ]; then
    docker tag "$TAG" "$VM_BASE_REGISTRY/$TAG"
    docker push "$VM_BASE_REGISTRY/$TAG"
    echo "== pushed $VM_BASE_REGISTRY/$TAG"
else
    echo "== built locally only — set VM_BASE_REGISTRY to tag+push"
fi
