$ErrorActionPreference = 'Stop'
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    Push-Location 'backend/components/lmstudio-client'
    try {
        npm ci --no-audit --no-fund
        if ($LASTEXITCODE) { throw 'npm ci failed' }
        npm run bundle
        if ($LASTEXITCODE) { throw 'SDK bundle failed' }
    } finally { Pop-Location }
    cargo test --locked
    if ($LASTEXITCODE) { throw 'Rust tests failed' }
    cargo build --release --locked
    if ($LASTEXITCODE) { throw 'Build failed' }
    New-Item -ItemType Directory -Force dist | Out-Null
    Copy-Item -LiteralPath 'target/release/supa-dev.exe' -Destination 'dist/supa-dev-windows-x86_64.exe'
    $sum = (Get-FileHash -LiteralPath 'dist/supa-dev-windows-x86_64.exe' -Algorithm SHA256).Hash.ToLowerInvariant()
    "$sum  supa-dev-windows-x86_64.exe" | Set-Content -LiteralPath 'dist/SHA256SUMS' -Encoding ASCII
} finally { Pop-Location }
