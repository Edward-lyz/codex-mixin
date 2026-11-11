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
EXPOSE 8317
ENTRYPOINT ["codex-mixin"]
CMD ["service", "start", "--foreground", "--bind", "0.0.0.0:8317"]
