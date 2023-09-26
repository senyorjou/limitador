# ------------------------------------------------------------------------------
# Build Stage
# ------------------------------------------------------------------------------

FROM registry.access.redhat.com/ubi8/ubi:8.7 as limitador-build
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

ARG RUSTC_VERSION=1.72.0

# the powertools repo is required for protobuf-c and protobuf-devel
RUN dnf -y --setopt=install_weak_deps=False --setopt=tsflags=nodocs install \
      http://mirror.centos.org/centos/8-stream/BaseOS/`arch`/os/Packages/centos-gpg-keys-8-6.el8.noarch.rpm \
      http://mirror.centos.org/centos/8-stream/BaseOS/`arch`/os/Packages/centos-stream-repos-8-6.el8.noarch.rpm \
 && dnf -y --setopt=install_weak_deps=False --setopt=tsflags=nodocs install epel-release \
 && dnf config-manager --set-enabled powertools

RUN PKGS="gcc-c++ gcc-toolset-12-binutils-gold openssl-devel protobuf-c protobuf-devel git clang kernel-headers" \
    && dnf install --nodocs --assumeyes $PKGS \
    && rpm --verify --nogroup --nouser $PKGS \
    && yum -y clean all

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --no-modify-path --profile minimal --default-toolchain ${RUSTC_VERSION} -c rustfmt -y

WORKDIR /usr/src/limitador

ARG GITHUB_SHA
ENV GITHUB_SHA=${GITHUB_SHA:-unknown}
ENV RUSTFLAGS="-C target-feature=-crt-static"

COPY . .

RUN source $HOME/.cargo/env \
    && cargo build --release
