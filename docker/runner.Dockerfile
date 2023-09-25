# ------------------------------------------------------------------------------
# Build & compile stage
# ------------------------------------------------------------------------------

FROM senyorjou/limitador-base as limitador-build

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
ARG RUSTC_VERSION=1.72.0


WORKDIR /usr/src/limitador

ARG GITHUB_SHA
ENV GITHUB_SHA=${GITHUB_SHA:-unknown}
ENV RUSTFLAGS="-C target-feature=-crt-static"

COPY . .

RUN source $HOME/.cargo/env \
    && cargo build --release

# ------------------------------------------------------------------------------
# Run Stage
# ------------------------------------------------------------------------------

FROM senyorjou/limitador-base-runner

WORKDIR /home/limitador/bin/
ENV PATH="/home/limitador/bin:${PATH}"

COPY --from=limitador-build /usr/src/limitador/limitador-server/examples/limits.yaml ../
COPY --from=limitador-build /usr/src/limitador/target/release/limitador-server ./limitador-server

RUN chown -R limitador:root /home/limitador \
    && chmod -R 750 /home/limitador

USER limitador

CMD ["limitador-server"]
