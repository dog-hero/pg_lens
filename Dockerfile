# pg_lens — multi-stage build: frontend (node) → static musl binary (rust)
# → minimal alpine runtime.
#
# The final stage is alpine (not scratch) on purpose: the services-file
# `password_cmd` feature executes via `sh -c`, so the image needs a shell.
#
# Build:  docker build -t pg_lens .
# Run:    docker run --rm -e PG_LENS_AUTH_TOKEN=... -p 8080:8080 pg_lens
# (serve on 0.0.0.0 refuses to start without PG_LENS_AUTH_TOKEN — by design.)

# --- Stage 1: web frontend bundle (dist/ is not committed) ----------------
FROM node:24-alpine AS frontend
WORKDIR /app
COPY crates/pg_lens_web/frontend/package.json crates/pg_lens_web/frontend/package-lock.json ./
RUN npm ci
COPY crates/pg_lens_web/frontend/ ./
RUN npm run build

# --- Stage 2: static musl binary (native target of rust:1-alpine) ---------
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY --from=frontend /app/dist/ crates/pg_lens_web/frontend/dist/
RUN cargo build --release -p pg_lens_tui \
    && cp target/release/pg_lens /pg_lens

# --- Final stage: alpine + the binary, non-root ----------------------------
FROM alpine:3.24
LABEL org.opencontainers.image.source="https://github.com/dog-hero/pg_lens" \
      org.opencontainers.image.description="A blazing-fast TUI and web dashboard for live PostgreSQL observability" \
      org.opencontainers.image.licenses="MIT"
COPY --from=builder /pg_lens /usr/local/bin/pg_lens
# nobody:nobody — the server never needs to write to the filesystem.
USER 65534:65534
EXPOSE 8080
ENTRYPOINT ["pg_lens"]
# Default: Web Lens on all interfaces. Requires PG_LENS_AUTH_TOKEN (the
# binary refuses non-loopback binds without it). Override the CMD for the
# TUI: `docker run -it --rm <image> tui --dsn ...`
CMD ["serve", "--listen", "0.0.0.0:8080"]
