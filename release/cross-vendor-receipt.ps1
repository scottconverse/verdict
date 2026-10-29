# SPEC-8 cross-vendor receipt harness
#
# Prepares (and, without -DryRun, runs) exactly ONE verifier desk from a vendor different from the
# builder, against a copy of the candidate project, using the desk protocol adapter — not a raw CLI.
# The build updates target/ only. Acceptance happens in a fresh temp copy with its own ledger,
# HOME and Verdict state; source, original ledger and user configuration remain unchanged.
#
#   pwsh -File release/cross-vendor-receipt.ps1 -DryRun      # plan + assertions, no model call
#   pwsh -File release/cross-vendor-receipt.ps1              # one authorized desk invocation
#
# Authorising a real run authorises exactly one desk call: one `verdict-desk/1` prompt to the
# configured adapter. Its existing bounded retry policy can invoke the CLI twice;
# the harness does not retry a failed desk. The receipt must not claim one CLI call.

param(
    # Perform every step except the desk (model) invocation.
    [switch]$DryRun,
    # Session being verified (default: the latest session in the copied ledger).
    [string]$Session = "",
    # Verifier vendor to configure; must differ from the builder's vendor.
    [string]$VerifierVendor = "openai",
    # Empty preserves the signed-in CLI's configured model; it is never served-identity proof.
    [string]$Model = ""
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "ledger-receipt-delta.ps1")
function Fail($msg) { Write-Host "refused: $msg" -ForegroundColor Red; exit 1 }
function Assert-True($cond, $msg) { if (-not $cond) { Fail "assertion failed: $msg" } }

# --- the entire candidate worktree must be clean, including this harness ------------------------
$repo = (& git rev-parse --show-toplevel 2>$null)
if (-not $repo) { Fail "run this from inside the candidate repository" }
$dirty = @(& git -C $repo status --porcelain | Where-Object { $_ -ne "" })
if ($LASTEXITCODE -ne 0) { Fail 'cannot inspect candidate status' }
if ($dirty.Count -gt 0) { Fail "the candidate worktree is dirty; commit or stash first: $($dirty -join '; ')" }
$candidate = (& git -C $repo rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) { Fail 'cannot identify candidate commit' }

# --- build from this candidate; do not silently consume stale debug binaries --------------------
& cargo build --release --locked --bin vdt --bin vdt-provider-codex
if ($LASTEXITCODE -ne 0) { Fail 'candidate binary build failed' }
$vdt = Join-Path $repo "target\release\vdt.exe"
if (-not (Test-Path $vdt)) { Fail "no built vdt.exe (run cargo build -p verdict-cli)" }
$adapter = Join-Path $repo "target\release\vdt-provider-codex.exe"
if (-not (Test-Path $adapter)) { Fail "no desk adapter (vdt-provider-codex) built; run cargo build -p verdict-desks --bins" }

# --- CLI version probes only; no model invocation -----------------------------------------------
$codex = (& cmd /C "codex --version" 2>$null); $codexOk = $LASTEXITCODE -eq 0
$claude = (& cmd /C "claude --version" 2>$null); $claudeOk = $LASTEXITCODE -eq 0
if ($VerifierVendor -ne "openai") { Fail "this harness configures the openai verifier (the codex desk adapter); got '$VerifierVendor'" }
if (-not $codexOk) { Fail "the codex CLI is not on PATH, so the openai desk adapter cannot answer" }

# --- isolated copy ------------------------------------------------------------------------------
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$root = Join-Path $env:TEMP "vdt-cross-vendor-$stamp-$([guid]::NewGuid().ToString('N').Substring(0,8))"
$proj = Join-Path $root "project"
$fakeHome = Join-Path $root "home"
$log = Join-Path $root "desk.log"
$receiptPath = Join-Path $root "receipt.txt"
New-Item -ItemType Directory -Path $proj, $fakeHome -Force | Out-Null
# Export precisely the committed candidate, excluding ignored private notes/configuration.
$sourceArchive = Join-Path $root 'candidate.tar'
& git -C $repo archive --format=tar --output=$sourceArchive $candidate
if ($LASTEXITCODE -ne 0) { Fail 'candidate archive failed' }
& tar -xf $sourceArchive -C $proj
if ($LASTEXITCODE -ne 0) { Fail 'candidate extraction failed' }
New-Item -ItemType Directory -Path (Join-Path $proj ".verdict") -Force | Out-Null
$backup = & python (Join-Path $PSScriptRoot 'copy-ledger.py') (Join-Path $repo '.verdict\ledger.db') (Join-Path $proj '.verdict\ledger.db')
if ($LASTEXITCODE -ne 0) { Fail "consistent ledger backup failed; retained diagnostic copy at $root" }
$backup | Set-Content (Join-Path $root 'ledger-backup.json')
$binDir = New-Item -ItemType Directory -Path (Join-Path $root 'bin')
Copy-Item -LiteralPath $vdt, $adapter -Destination $binDir.FullName
$vdt = Join-Path $binDir.FullName 'vdt.exe'
$adapter = Join-Path $binDir.FullName 'vdt-provider-codex.exe'
$binaryHashes = Get-FileHash -LiteralPath $vdt, $adapter -Algorithm SHA256 | Select-Object Path, Hash
if ((& git -C $repo rev-parse HEAD).Trim() -ne $candidate -or @(& git -C $repo status --porcelain).Count -ne 0) {
    Fail 'candidate changed during build/copy; refuse model execution'
}
@{ candidate=$candidate; binaries=$binaryHashes; ledger=($backup | ConvertFrom-Json); requested_model=$Model } |
    ConvertTo-Json -Depth 10 | Set-Content (Join-Path $root 'provenance.json')

# --- the session being verified, and its builder vendor (no inference, no skip) ----------------
Push-Location $proj
try {
    & $vdt ledger verify
    if ($LASTEXITCODE -ne 0) { Fail 'copied ledger hash-chain verification failed' }
    $sessions = (& $vdt sessions | Out-String)
    if ($LASTEXITCODE -ne 0) { Fail 'cannot read copied sessions' }
} finally { Pop-Location }
if (-not $Session) {
    $m = [regex]::Matches($sessions, 's-[0-9a-f]+')
    if ($m.Count -eq 0) { Fail "no session in the copied ledger to verify" }
    $Session = $m[$m.Count - 1].Value
}
$line = ($sessions -split "`n" | Where-Object { $_ -match [regex]::Escape($Session) }) -join " "
if (-not $line) { Fail "session $Session is not in the copied ledger" }
$m = [regex]::Match($line, 'vendor\s+([A-Za-z0-9_.-]+)')
if (-not $m.Success) { Fail "cannot read the builder's vendor for session $Session; refusing to guess" }
$builderVendor = $m.Groups[1].Value
if (-not $builderVendor) { Fail "the builder's vendor for $Session is empty; cross-vendor cannot be asserted" }
if ($builderVendor -ieq $VerifierVendor) {
    Fail "verifier vendor '$VerifierVendor' equals the builder's vendor '$builderVendor' for $Session"
}

# --- configure one adapter provider and one desk, consistently with verifier_order --------------
$policyPath = Join-Path $proj "verdict.json"
$policy = Get-Content $policyPath -Raw | ConvertFrom-Json
$policy.vendors.providers = @(
    [pscustomobject]@{
        name         = "cross-vendor-codex"
        vendor       = $VerifierVendor
        command      = $adapter
        args         = @()
        model        = $Model
        timeout_secs = 300
    }
)
$order = @($policy.vendors.verifier_order | Where-Object { $_ })
if ($order -notcontains $VerifierVendor) { $order = @($VerifierVendor) }
$policy.vendors.verifier_order = $order
$policy.lanes.standard.desks = @($policy.lanes.standard.desks[0])
($policy | ConvertTo-Json -Depth 30) | Set-Content $policyPath -NoNewline

$written = Get-Content $policyPath -Raw | ConvertFrom-Json
$desks = @($written.lanes.standard.desks)
$providers = @($written.vendors.providers)
$orderNow = @($written.vendors.verifier_order)

# --- assertions: exactly one desk, one matching provider, vendor ordered -----------------------
Assert-True ($desks.Count -eq 1) "the standard lane must use exactly one desk (got $($desks.Count))"
Assert-True ($providers.Count -eq 1) "exactly one provider must be configured (got $($providers.Count))"
Assert-True ($providers[0].vendor -eq $VerifierVendor) "the provider vendor must be $VerifierVendor"
Assert-True ($providers[0].command -ieq $adapter) "the provider command must be the desk adapter at $adapter"
Assert-True ($orderNow -contains $VerifierVendor) "verifier_order must contain $VerifierVendor"
Assert-True ($builderVendor -ine $VerifierVendor) "builder and verifier vendors must differ"

$command = "$vdt desks --lane standard --session $Session --project `"$proj`""

Write-Host ""
Write-Host "Cross-vendor desk receipt — plan"
Write-Host "  candidate repo : $repo (clean; untouched)"
Write-Host "  session        : $Session (builder vendor $builderVendor)"
Write-Host "  temp copy      : $proj (own ledger; HOME=$fakeHome)"
Write-Host "  verifier       : desk adapter $adapter -> codex ($VerifierVendor), requested model '$Model' (empty preserves CLI default)"
Write-Host "  CLIs present   : codex=$(if ($codexOk) { $codex } else { 'absent' }); claude=$(if ($claudeOk) { $claude } else { 'absent' })"
Write-Host "  desk protocol  : verdict-desk/1 JSON request, answered by the adapter (not a raw CLI call)"
Write-Host "  desks          : 1 ($($desks[0])); providers: 1; verifier_order: $($orderNow -join ', ')"
Write-Host "  model calls    : 1 desk invocation; adapter permits at most two CLI attempts"
Write-Host "  command        : $command"
Write-Host "  receipt        : $receiptPath (under the temp root; never in the repo)"
Write-Host "  assertions     : exactly 1 desk in the lane, exactly 1 provider for $VerifierVendor whose"
Write-Host "                   command is the adapter, verifier_order contains $VerifierVendor,"
Write-Host "                   builder $builderVendor != verifier $VerifierVendor, and after the run"
Write-Host "                   exactly 1 NEW desk record for this session and desk, retaining all prior history."
Write-Host ""

# Snapshot copied history before any authorized desk call. Never clear it.
Push-Location $proj
try {
    $beforeExport = (& $vdt ledger export --jsonl | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'Baseline ledger export failed' }
} finally { Pop-Location }
$beforeRecords = @(Read-LedgerReceiptRows $beforeExport)

if ($DryRun) {
    Write-Host "dry run: all of the above was actually done (copy, policy, assertions); no model was invoked."
    exit 0
}

# --- the one authorised run ---------------------------------------------------------------------
# Keep the signed-in CLI's supported config location. Never copy credentials into evidence.
$savedEnv = @{}
foreach ($key in @('HOME','USERPROFILE','VERDICT_STATE_DIR','CODEX_HOME')) { $savedEnv[$key] = [Environment]::GetEnvironmentVariable($key) }
try {
    if (-not $env:CODEX_HOME) { $env:CODEX_HOME = Join-Path $env:USERPROFILE '.codex' }
    $env:HOME = $fakeHome
    $env:USERPROFILE = $fakeHome
    $env:VERDICT_STATE_DIR = (Join-Path $root 'state')
    & $vdt desks --lane standard --session $Session --project $proj 2>&1 | Tee-Object -FilePath $log | Write-Host
    $deskExit = $LASTEXITCODE
} finally {
    foreach ($key in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($key, $savedEnv[$key]) }
}
if ($deskExit -ne 0) { Fail "desk run exited $deskExit; no automatic retry; see $log" }

# --- receipt from the temp ledger, with the post-run assertion ----------------------------------
Push-Location $proj
try {
    $export = (& $vdt ledger export --jsonl | Out-String)
    if ($LASTEXITCODE -ne 0) { throw 'Post-run ledger export failed' }
} finally { Pop-Location }
$afterRecords = @(Read-LedgerReceiptRows $export)
$newReceipt = Assert-AppendOnlyDeskReceipt $beforeRecords $afterRecords $Session $desks[0]
$deskRecords = @($newReceipt)
$deskLine = $newReceipt | ConvertTo-Json -Depth 100 -Compress
$receipt = @(
    "Cross-vendor desk receipt",
    "timestamp: $(Get-Date -Format o)",
    "candidate repo (clean, untouched): $repo",
    "session: $Session",
    "builder vendor: $builderVendor",
    "candidate commit: $candidate",
    "verifier: desk adapter $adapter -> codex / $VerifierVendor / requested model '$Model'",
    "command: $command",
    "desk log: $log",
    "new desk records: $($deskRecords.Count)",
    "preserved prior records: $($beforeRecords.Count)",
    "new desk sequence: $($newReceipt.seq)",
    "desk record: $deskLine"
) -join "`n"
$receipt | Set-Content $receiptPath -NoNewline
Write-Host "wrote $receiptPath"
