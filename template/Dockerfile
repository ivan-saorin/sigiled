# vm-tmpl v2 (DEC-17): thin project image. The workspace agent lives in the
# pre-built base image — adopting a new agent = bumping the tag below (keep
# it in sync with the `template = "vm-tmpl@x.y.z"` pin in mgr.toml).
# The image is public on ghcr (DEC-20): pulls need no credentials. A self-host
# that builds its own base swaps the registry prefix, nothing else.
FROM ghcr.io/ivan-saorin/vm-base:0.1.0

# --- project toolchain layers (project-owned, edit freely) -----------------
# The base runtime is debian-slim + git/bash/curl and nothing else. A project
# that needs a toolchain adds its own layers here, e.g.:
#
#   USER root
#   RUN apt-get update && apt-get install -y --no-install-recommends \
#       python3 python3-venv && rm -rf /var/lib/apt/lists/*
#   USER 1000:1000
#
# A project with ext-py/ or ext-go/ (DEC-18) brings the matching runtime in
# these layers; ext-rust/ needs nothing here (compiled into vm-base).
# ---------------------------------------------------------------------------
