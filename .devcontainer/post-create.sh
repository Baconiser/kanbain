#!/usr/bin/env bash
set -euo pipefail

# Named volumes are created root-owned
sudo chown -R "$(id -u):$(id -g)" "${PWD}/target" /usr/local/cargo/registry

# System libraries: bindgen (clang) + Tauri (webkit2gtk etc.)
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  clang libclang-dev libssl-dev pkg-config \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev
sudo rm -rf /var/lib/apt/lists/*

# Install the toolchain pinned in rust-toolchain.toml
rustup show

# Cargo tools used by package.json scripts (SQLite only, no Postgres)
cargo install cargo-watch --locked
cargo install sqlx-cli --no-default-features --features sqlite,rustls --locked

# System libraries for headless Chromium (the browser itself comes with the run skill)
sudo env "PATH=$PATH" npx -y playwright install-deps chromium

# pnpm version comes from package.json "packageManager"
sudo corepack enable
corepack install
pnpm install
