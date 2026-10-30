# Local-only native tests; no administrator rights or firewall policy changes.
$ErrorActionPreference = 'Stop'
$previousLoopback = $env:AICONNECT_TEST_LOOPBACK
try {
    $env:AICONNECT_TEST_LOOPBACK = '1'
    cargo test --locked --workspace
    if ($LASTEXITCODE -ne 0) { throw 'Rust tests failed' }
} finally {
    if ($null -eq $previousLoopback) { Remove-Item Env:AICONNECT_TEST_LOOPBACK -ErrorAction SilentlyContinue }
    else { $env:AICONNECT_TEST_LOOPBACK = $previousLoopback }
}
