# Two roles, one file:
# 1. Session image of the sigiled repo itself under MGR v1 (which builds the
#    project image from the repo root Dockerfile) — v1 compat until cutover.
# 2. Source of the published base image `vm-base:x.y.z` (DEC-17), built and
#    tagged by images/build-vm-base.sh.
FROM rust:1.97.1-slim AS build
WORKDIR /src
COPY . .
RUN ./vm-base/build-ext.sh && cargo build --release --locked -p vm-base

FROM debian:13.6-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git openssh-client ca-certificates bash curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 1000 -m dev \
    && mkdir -p /workspace /secrets && chown 1000:1000 /workspace /secrets
COPY --from=build /src/target/release/vm-base /usr/local/bin/vm-base

USER 1000:1000
WORKDIR /workspace
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["vm-base", "health-probe"]
CMD ["vm-base"]
