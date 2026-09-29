$ErrorActionPreference = 'Stop'
$sourceRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Push-Location $sourceRoot
try {
    docker run --rm `
        -v "${sourceRoot}:/src" `
        -v rustdesk-server-cargo:/usr/local/cargo/registry `
        -v rustdesk-server-cargo-git:/usr/local/cargo/git `
        -v rustdesk-server-target:/src/target `
        -w /src `
        rust:1.90-bookworm@sha256:3914072ca0c3b8aad871db9169a651ccfce30cf58303e5d6f2db16d1d8a7e58f `
        sh scripts/verify-relay.sh
    if ($LASTEXITCODE -ne 0) { throw 'Relay build or tests failed.' }
    docker build -f Dockerfile.relay-local -t rustdesk-server:relay-local .
    if ($LASTEXITCODE -ne 0) { throw 'Relay image build failed.' }
} finally {
    Pop-Location
}
