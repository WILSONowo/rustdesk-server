#!/bin/sh
set -eu
source_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$source_root"
docker run --rm \
    -v "$source_root:/src" \
    -v rustdesk-server-cargo:/usr/local/cargo/registry \
    -v rustdesk-server-cargo-git:/usr/local/cargo/git \
    -v rustdesk-server-target:/src/target \
    -w /src \
    rust:1.90-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f \
    sh scripts/verify-relay.sh
docker build -f Dockerfile.relay-local -t rustdesk-server:relay-local .
