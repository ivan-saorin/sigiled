#!/bin/sh
# Build (and optionally push) the base workspace image vm-base:x.y.z (DEC-17).
# Run on the box or any docker host — SIGILED workspaces have no docker: when a
# session bumps the agent, the OPERATOR runs this and pushes.
#
#   images/build-vm-base.sh              # build ghcr.io/ivan-saorin/vm-base:<version>
#   PUSH=1 images/build-vm-base.sh       # + push (needs docker login ghcr.io)
#   VM_BASE_REGISTRY=reg.example.com images/build-vm-base.sh   # self-host registry
#
# The registry defaults to the public ghcr namespace (DEC-20) so the tag
# printed here is exactly what template/Dockerfile pins in its FROM line.
# Version comes from vm-base/Cargo.toml — single source of truth for the tag
# that template/Dockerfile and the mgr.toml pin refer to.
set -eu

cd "$(dirname "$0")/.."
VER=$(grep -m1 '^version' vm-base/Cargo.toml | cut -d'"' -f2)
[ -n "$VER" ] || { echo "cannot read version from vm-base/Cargo.toml"; exit 1; }
REG="${VM_BASE_REGISTRY:-ghcr.io/ivan-saorin}"
TAG="$REG/vm-base:$VER"
SHA=$(git rev-parse --short=12 HEAD 2>/dev/null || echo worktree)

echo "== building $TAG (repo @ $SHA)"
docker build -t "$TAG" .

if [ -n "${PUSH:-}" ]; then
    docker push "$TAG"
    echo "== pushed $TAG"
else
    echo "== built locally only — set PUSH=1 to push"
fi
