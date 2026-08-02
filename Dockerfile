FROM rust:1.97.1-slim AS build
WORKDIR /src
COPY . .
RUN ./build-ext.sh && cd server && cargo build --release --locked

FROM debian:13.6-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git openssh-client ca-certificates bash curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -u 1000 -m dev \
    && mkdir -p /workspace /secrets && chown 1000:1000 /workspace /secrets
COPY --from=build /src/server/target/release/vm-base /usr/local/bin/vm-base

USER 1000:1000
WORKDIR /workspace
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["vm-base", "health-probe"]
CMD ["vm-base"]
