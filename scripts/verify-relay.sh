#!/bin/sh
set -eu
cp db_v2.sqlite3 /tmp/hbbs-relay-build-schema.sqlite3
export DATABASE_URL=sqlite:///tmp/hbbs-relay-build-schema.sqlite3
cargo test --locked --lib relay_scheduler::tests
cargo test --locked --lib secure_tcp::tests
cargo build --locked --release --bin hbbs --bin hbbr
export HBBS_TEST_BINARY=/src/target/release/hbbs
export HBBR_TEST_BINARY=/src/target/release/hbbr
cargo test --locked --test secure_handshake --test relay_scheduler
mkdir -p artifacts/relay
cp target/release/hbbs target/release/hbbr artifacts/relay/
sha256sum artifacts/relay/hbbs artifacts/relay/hbbr > artifacts/relay/SHA256SUMS
