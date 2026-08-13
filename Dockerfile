# xiaoO container image — openEuler 24.03 LTS SP3 (builder + runtime).
# Builds the two binaries (xiaoo, moirai) plus the skills / hookers / docs
# trees and packages them with every runtime dependency preinstalled. Ships
# NO config.toml — mount your own at run. Default CMD launches `xiaoo`.
# Build & run instructions: docs/docker_deploy.md.
###############################################################################

# ── Stage 1 — builder: compile binaries ─────────────────────────────────────
FROM openeuler/openeuler:24.03-lts-sp3 AS builder

# Native build deps + Rust toolchain in one dnf install (no rustup needed).
# openEuler 24.03 SP3 update repo ships rust/cargo 1.90.0; dnf already uses
# the Huawei Cloud mirror by default — fast in China.
RUN dnf install -y \
        gcc \
        gcc-c++ \
        make \
        cmake \
        pkg-config \
        perl \
        openssl-devel \
        curl \
        ca-certificates \
        tar \
        rust \
        cargo \
    && dnf clean all \
    && rm -rf /var/cache/dnf

ENV CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN rustc --version && cargo --version

# Repo's bundled .cargo/config.toml (Huawei Cloud crates.io mirror) as the
# global cargo config — fast downloads in China, no build-arg needed.
COPY .cargo/config.toml /usr/local/cargo/config.toml

WORKDIR /build

# Layered COPY: copy every workspace Cargo.toml + Cargo.lock, run `cargo fetch`
# first, then copy real source. Code changes won't invalidate the fetch layer
# — only Cargo.toml/Cargo.lock changes will.
COPY Cargo.toml Cargo.lock ./
COPY apps/shared/Cargo.toml           apps/shared/
COPY apps/endside/Cargo.toml          apps/endside/
COPY apps/vault/Cargo.toml            apps/vault/
COPY crates/agent-contracts/Cargo.toml    crates/agent-contracts/
COPY crates/agent-llm/Cargo.toml          crates/agent-llm/
COPY crates/agent-types/Cargo.toml        crates/agent-types/
COPY crates/compact/Cargo.toml            crates/compact/
COPY crates/core/Cargo.toml               crates/core/
COPY crates/hook/Cargo.toml               crates/hook/
COPY crates/llm-client/Cargo.toml         crates/llm-client/
COPY crates/llm-client-cli/Cargo.toml     crates/llm-client-cli/
COPY crates/lsp/Cargo.toml                crates/lsp/
COPY crates/mcp/Cargo.toml                crates/mcp/
COPY crates/memory/Cargo.toml            crates/memory/
COPY crates/operation_backend/Cargo.toml  crates/operation_backend/
COPY crates/prompt/Cargo.toml             crates/prompt/
COPY crates/skill/Cargo.toml              crates/skill/
COPY crates/subagent/Cargo.toml           crates/subagent/
COPY crates/tool/Cargo.toml               crates/tool/
COPY crates/trace/Cargo.toml              crates/trace/
COPY crates/trace/src/moirai/Cargo.toml   crates/trace/src/moirai/

# Placeholder source files so cargo can resolve workspace metadata for
# `cargo fetch` (some Cargo.toml files declare `[[bin]] path = ...`).
# Overwritten by the next COPY . .
RUN find . -name Cargo.toml | while read f; do \
        dir="$(dirname "$f")"; \
        awk -F'"' '/^[[:space:]]*path[[:space:]]*=/ {print $2}' "$f" | while read p; do \
            full="$dir/$p"; \
            mkdir -p "$(dirname "$full")"; \
            [ -f "$full" ] || case "$p" in \
                *main.rs|*bin/*) printf 'fn main() {}\n' > "$full" ;; \
                *) : > "$full" ;; \
            esac; \
        done; \
        mkdir -p "$dir/src"; \
        [ -f "$dir/src/main.rs" ] || printf 'fn main() {}\n' > "$dir/src/main.rs"; \
        [ -f "$dir/src/lib.rs" ] || : > "$dir/src/lib.rs"; \
    done

# Download all deps (no compile). Cargo.lock is copied in as a baseline; if it
# has drifted against any Cargo.toml, cargo silently updates it here so the
# build self-heals instead of failing. --locked would break the build on drift.
RUN cargo fetch

# Full source tree (.dockerignore keeps target/, .git/, config.toml out).
COPY . .

# Build the two binaries. Without buildx cache mounts, target/ is rebuilt
# from scratch each docker build — but fetch above ensures deps are already
# downloaded, so only compilation work remains.
RUN cargo build --release -p xiaoo-endside -p moirai

# ── Stage packaged files at /staging/ ────────────────────────────────────────

# Binaries → /usr/bin/
RUN mkdir -p /staging/usr/bin && \
    cp /build/target/release/xiaoo  /staging/usr/bin/xiaoo && \
    cp /build/target/release/moirai /staging/usr/bin/moirai

# Skills → /usr/lib/.xiaoo/skills/
RUN mkdir -p /staging/usr/lib/.xiaoo/skills && \
    cp -a /build/plugins/skills/* /staging/usr/lib/.xiaoo/skills/

# Hookers → /usr/lib/.xiaoo/hookers/. Three transformations:
#   1) rm audit_agent/install.sh  — Python deps come from system, not venv
#   2) rewrite audit_agent/plugin.json to call /usr/bin/python3 with absolute
#      paths so audit_agent works without a per-user venv
#   3) chmod +x every install.sh / uninstall.sh (for xiaoo-hookers-install)
RUN mkdir -p /staging/usr/lib/.xiaoo/hookers && \
    cp -a /build/plugins/hookers/* /staging/usr/lib/.xiaoo/hookers/ && \
    rm -f /staging/usr/lib/.xiaoo/hookers/audit_agent/install.sh && \
    printf '%s\n' \
      '[' \
      '  {' \
      '    "id": "plugin_audit_tool_input",' \
      '    "hook_point": "*.Tool.*.pre",' \
      '    "command": "PYTHONPATH=/usr/lib/.xiaoo/hookers/audit_agent/audit_policy_checker /usr/bin/python3 /usr/lib/.xiaoo/hookers/audit_agent/audit.py"' \
      '  }' \
      ']' > /staging/usr/lib/.xiaoo/hookers/audit_agent/plugin.json && \
    find /staging/usr/lib/.xiaoo/hookers \( -name install.sh -o -name uninstall.sh \) -exec chmod +x {} +

# Docs → /usr/share/doc/xiaoO/ (README.md, README.zh-CN.md, docs/*.md)
RUN mkdir -p /staging/usr/share/doc/xiaoO && \
    cp /build/README.md        /staging/usr/share/doc/xiaoO/README.md && \
    cp /build/README.zh-CN.md  /staging/usr/share/doc/xiaoO/README.zh-CN.md && \
    cp /build/docs/*.md        /staging/usr/share/doc/xiaoO/

# License → /usr/share/licenses/xiaoO/
RUN mkdir -p /staging/usr/share/licenses/xiaoO && \
    cp /build/License/LICENSE                       /staging/usr/share/licenses/xiaoO/LICENSE && \
    cp /build/License/Third_Party_Open_Source_Software_Notice.md \
       /staging/usr/share/licenses/xiaoO/ 2>/dev/null || true

# ── Stage 2 — runtime: install deps, copy staged files ──────────────────────
FROM openeuler/openeuler:24.03-lts-sp3 AS runtime

# Runtime deps via dnf. openEuler's container image enables OS / everything /
# EPOL(main) / update by default, so all packages below resolve directly —
# NO pip needed. Versions meet the apps' >= constraints (e.g. fastapi 0.115.12,
# uvicorn 0.34.0, starlette 0.46.1).
RUN dnf install -y \
        ca-certificates \
        curl \
        git \
        xclip \
        python3 \
        python3-openai \
        python3-httpx \
        python3-pydantic \
        python3-tenacity \
        python3-tomli \
        python3-fastapi \
        python3-uvicorn \
        python3-starlette \
    && dnf clean all \
    && rm -rf /var/cache/dnf

# Copy the entire staged tree (binaries, skills, hookers, docs, license).
COPY --from=builder /staging/ /

# Pre-create runtime dirs so `-v file:.../config.toml` mounts work without
# a parent VOLUME; also wire the hookers install/uninstall symlinks.
RUN mkdir -p /root/.xiaoo /root/.config/xiaoo && \
    ln -sf /usr/lib/.xiaoo/hookers/install.sh   /usr/bin/xiaoo-hookers-install && \
    ln -sf /usr/lib/.xiaoo/hookers/uninstall.sh /usr/bin/xiaoo-hookers-uninstall

# XIAOO_CONFIG is intentionally NOT set — operator passes it at `docker run`.
ENV RUST_LOG=info \
    HOME=/root

# Declared VOLUME so a read-only config mount on /root/.config/xiaoo stays clean.
VOLUME ["/root/.xiaoo"]

# No daemon ports / healthcheck: xiaoo is a TUI/CLI, not an HTTP server. The
# previous EXPOSE 18080 28081 and HEALTHCHECK targeted the now-removed
# xiaoo-daemon HTTP API.

WORKDIR /root

# No ENTRYPOINT: `xiaoo` runs directly as PID 1 (exec-form CMD, no shell
# wrapper). The TUI needs a TTY — run with `docker run -it`. For the CLI /
# bash dispatch forms see docs/docker_deploy.md.
CMD ["xiaoo"]
