# Build stage: templates and static files are embedded at compile time.
FROM rust:1.99.1-slim AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY settings.toml ./
COPY src ./src
COPY templates ./templates
COPY static ./static
RUN cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home searxng
WORKDIR /app
COPY --from=build /app/target/release/rustxng /usr/local/bin/rustxng
COPY --from=build /app/settings.toml /app/settings.toml
COPY LICENSE NOTICE /app/
USER searxng
ENV SEARXNG_SETTINGS_PATH=/app/settings.toml
EXPOSE 8888
ENTRYPOINT ["/usr/local/bin/rustxng"]
CMD ["--bind", "0.0.0.0", "--port", "8888"]
