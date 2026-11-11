# Runtime image built from the static musl release binaries.
# Build context must contain the extracted binaries as
# dist/docker/{amd64,arm64}/codex-mixin (see the release workflow).
FROM alpine:3.22
ARG TARGETARCH
RUN apk add --no-cache ca-certificates tzdata \
    && adduser -D -h /data codex
COPY dist/docker/${TARGETARCH}/codex-mixin /usr/local/bin/codex-mixin
USER codex
ENV HOME=/data
WORKDIR /data
VOLUME ["/data"]
# 8317: gateway API. 8318: web control panel (its token is printed to the logs).
EXPOSE 8317 8318
# The gateway runs as a daemon; the panel stays in the foreground so the
# container lives as long as the panel and `docker logs` shows the panel URL.
CMD ["sh", "-c", "codex-mixin service start --bind 0.0.0.0:8317 && exec codex-mixin panel --bind 0.0.0.0:8318"]
