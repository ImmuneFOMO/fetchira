# syntax=docker/dockerfile:1.7
FROM node:22-bookworm-slim AS webui-build
WORKDIR /src
COPY webui ./webui
COPY tools ./tools
RUN node tools/build_webui.js

FROM rust:1.94-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang perl pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY --from=webui-build /src/webui ./webui
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release && cp target/release/fetchira /fetchira

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget xvfb \
    && ARCH="$(dpkg --print-architecture)" \
    && wget -qO /tmp/chrome.deb "https://dl.google.com/linux/direct/google-chrome-stable_current_${ARCH}.deb" \
    && apt-get install -y --no-install-recommends /tmp/chrome.deb \
    && rm /tmp/chrome.deb && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin fetchira \
    && install -d -o fetchira -g fetchira /data
COPY --from=build /fetchira /usr/local/bin/fetchira
COPY deploy/container-entrypoint.sh /usr/local/bin/fetchira-entrypoint.sh
RUN chmod 0755 /usr/local/bin/fetchira-entrypoint.sh
USER fetchira
# Hosted is self-contained: web providers never depend on a browser installed on the host.
# The container is the isolation boundary, so the ChatGPT CDP worker uses Chrome's
# container mode (the application adds --no-sandbox only when this flag is set).
# Headful Chrome on Xvfb, not Chromium headless: image-heavy ChatGPT pages rendered
# differently under --headless=new and wedged the renderer on hosted servers. Xvfb gives
# the browser a real display while keeping the box headless.
ENV FETCHIRA_HOME=/data \
    FETCHIRA_BIND=0.0.0.0:7879 \
    FETCHIRA_CONTAINER=1 \
    FETCHIRA_BROWSER=chrome \
    FETCHIRA_CHROMIUM_BIN=/usr/bin/google-chrome-stable \
    FETCHIRA_BROWSER_HEADFUL=1 \
    FETCHIRA_REQUIRE_BROWSER=1 \
    DISPLAY=:99
VOLUME ["/data"]
EXPOSE 7879
ENTRYPOINT ["/usr/local/bin/fetchira-entrypoint.sh"]
CMD ["server"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD ["/data/fetchira", "server", "healthcheck"]
