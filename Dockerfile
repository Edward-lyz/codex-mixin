# Runtime image built from the static musl release binaries.
# Build context must contain the extracted binaries as
# dist/docker/{amd64,arm64}/codex-mixin (see the release workflow).
FROM alpine:3.22
ARG TARGETARCH
RUN apk add --no-cache ca-certificates tzdata tini \
    && adduser -D -h /data codex
COPY dist/docker/${TARGETARCH}/codex-mixin /usr/local/bin/codex-mixin
USER codex
ENV HOME=/data
WORKDIR /data
VOLUME ["/data"]
# 8317: gateway API. 8318: web control panel (its token is printed to the logs).
EXPOSE 8317 8318
# `service start` forks the gateway daemon and the panel stays in the
# foreground, so PID 1 must reap orphans and forward signals itself. tini does
# that, which makes `docker run --init` / compose `init: true` unnecessary.
ENTRYPOINT ["/sbin/tini", "--"]
# The panel stays in the foreground so the container lives as long as the panel
# and `docker logs` shows the panel URL. A fresh volume has no provider, so
# `service start` fails until one is added in the panel; retry in the
# background instead of exiting, and bind 0.0.0.0 so the saved endpoint is
# reachable from outside the container. The loop is double-forked so tini, not
# the panel, reaps it when it finishes.
CMD ["sh", "-c", "( (until codex-mixin service start --bind 0.0.0.0:8317; do echo 'gateway not started yet: add a provider in the panel; retrying in 5s' >&2; sleep 5; done) & ); exec codex-mixin panel --bind 0.0.0.0:8318"]
