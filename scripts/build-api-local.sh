#!/bin/sh
set -eu
source_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$source_root"
mkdir -p artifacts
docker run --rm \
    -v "$source_root:/src" \
    -v rustdesk-server-cargo:/usr/local/cargo/registry \
    -v rustdesk-server-cargo-git:/usr/local/cargo/git \
    -v rustdesk-server-target:/src/target \
    -w /src \
    rust:1.90-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f \
    sh -ec 'cp db_v2.sqlite3 /tmp/hbbs-build-schema.sqlite3
        export DATABASE_URL=sqlite:///tmp/hbbs-build-schema.sqlite3
        cargo test --locked --lib secure_tcp::tests
        cargo build --locked --release --bin hbbs
        HBBS_TEST_BINARY=/src/target/release/hbbs cargo test --locked --test secure_handshake
        cp target/release/hbbs artifacts/hbbs'
docker build -f Dockerfile.api-local -t rustdesk-hbbs:1.1.16-beta1-local .
sha256sum artifacts/hbbs
