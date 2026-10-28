# KanbAIn agent base image. Every agent run gets a container from this image
# (or from a project Dockerfile that starts `FROM kanbain-agent:<ver>`).
# The backend builds it on first use; the CLI versions come from
# crates/executors/src/versions.rs as build args.
FROM node:22-bookworm

ARG CLAUDE_CODE_PACKAGE
ARG CLAUDE_CODE_VERSION
ARG OPENCODE_PACKAGE
ARG OPENCODE_VERSION

# git, python3, gcc and make come with the base image
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        socat ripgrep jq less procps python3-pip python3-venv pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /workspace /kanbain/context /kanbain/out \
    && chown node:node /workspace /kanbain/out

USER node
ENV HOME=/home/node \
    PATH=/home/node/.cargo/bin:$PATH \
    npm_config_update_notifier=false \
    npm_config_fund=false \
    npm_config_prefer_offline=true \
    DISABLE_AUTOUPDATER=1

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain stable --component clippy,rustfmt

# Warm the npx cache with the exact commands the executors run, so runs start
# without downloading the CLIs.
RUN npx -y "${CLAUDE_CODE_PACKAGE}@${CLAUDE_CODE_VERSION}" --version \
    && npx -y "${OPENCODE_PACKAGE}@${OPENCODE_VERSION}" --version

RUN git config --global init.defaultBranch main \
    && git config --global user.name "KanbAIn agent" \
    && git config --global user.email "agent@kanbain.local"

WORKDIR /workspace
CMD ["sleep", "infinity"]
