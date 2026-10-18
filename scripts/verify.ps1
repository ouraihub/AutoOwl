$ErrorActionPreference = 'Stop'

pnpm check
pnpm lint
pnpm test
& "$PSScriptRoot\cargo-msvc.bat" fmt -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& "$PSScriptRoot\cargo-msvc.bat" clippy --all-targets --all-features -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& "$PSScriptRoot\cargo-msvc.bat" test
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
pnpm build
