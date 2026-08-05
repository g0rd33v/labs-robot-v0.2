# The robot in a container (§2: one binary, its data beside it).
#
# Two stages so the runtime image carries no toolchain: the build stage
# compiles against the same glibc the runtime uses, and the final image is
# the binary plus the TLS/SQLCipher shared libraries it links.
#
# The data directory is NOT in the image. It is a mounted volume, because
# the data IS the robot (§8) — an image is a program, and a program is the
# replaceable half.
FROM rust:1.97-trixie AS build
WORKDIR /src
# system deps for SQLCipher's vendored OpenSSL build
# TRIXIE, not bookworm: fastembed pulls a PREBUILT ONNX Runtime, and that
# binary is linked against glibc 2.38+/libstdc++ 13. On bookworm it fails
# with undefined __isoc23_strtoll and _M_replace_cold -- a base-image
# mismatch wearing a linker error's clothes. g++ is for the C++ symbols
# ONNX needs on the link line.
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev perl make g++ \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTFLAGS="-C link-arg=-lstdc++"
COPY Cargo.toml Cargo.lock ./
COPY trust/ trust/
COPY prism/ prism/
COPY mind/ mind/
COPY soul/ soul/
COPY hub/ hub/
COPY surfaces/ surfaces/
COPY robotd/ robotd/
RUN cargo build --release --bin robotd

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/robotd /usr/local/bin/robotd
WORKDIR /robot
# 7777 inside; the proxy maps the public path to it
EXPOSE 7777
ENTRYPOINT ["/usr/local/bin/robotd"]
CMD ["serve", "--config", "/robot/robot.toml"]
