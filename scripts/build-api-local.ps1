$ErrorActionPreference = 'Stop'
$sourceRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
New-Item -ItemType Directory -Force (Join-Path $sourceRoot 'artifacts') | Out-Null
Push-Location $sourceRoot
try {
    docker run --rm `
        -v "${sourceRoot}:/src" `
        -v rustdesk-server-cargo:/usr/local/cargo/registry `
        -v rustdesk-server-cargo-git:/usr/local/cargo/git `
        -v rustdesk-server-target:/src/target `
        -w /src `
        rust:1.90-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f `
        sh -c 'cp db_v2.sqlite3 /tmp/hbbs-build-schema.sqlite3 && export DATABASE_URL=sqlite:///tmp/hbbs-build-schema.sqlite3 && cargo test --locked --lib secure_tcp::tests && cargo build --locked --release --bin hbbs && HBBS_TEST_BINARY=/src/target/release/hbbs cargo test --locked --test secure_handshake && cp target/release/hbbs artifacts/hbbs'
    if ($LASTEXITCODE -ne 0) { throw 'Build or protocol tests failed.' }
    docker build -f Dockerfile.api-local -t rustdesk-hbbs:1.1.16-beta1-local .
    if ($LASTEXITCODE -ne 0) { throw 'Docker image build failed.' }
    Get-FileHash (Join-Path $sourceRoot 'artifacts/hbbs') -Algorithm SHA256
} finally {
    Pop-Location
}
