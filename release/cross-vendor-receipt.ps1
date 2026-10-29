# SPEC-8 cross-vendor receipt harness
#
# Prepares (and, without -DryRun, runs) exactly ONE verifier desk from a vendor different from the
# builder, against a copy of the candidate project, using the desk protocol adapter — not a raw CLI.
# Nothing here touches `merge-trial`: the work happens in a fresh temp copy with its own ledger, HOME
# and state, and the receipt is written under that temp root, never into the repository.
#
#   pwsh -File release/cross-vendor-receipt.ps1 -DryRun      # plan + assertions, no model call
#   pwsh -File release/cross-vendor-receipt.ps1              # the one authorised call
#
# Authorising a real run authorises exactly one desk call: one `verdict-desk/1` prompt to the
# configured adapter, which itself invokes the verifier CLI once.

param(
    # Perform every step except the desk (model) invocation.
    [switch]$DryRun,
    # Session being verified (default: the latest session in the copied ledger).
    [string]$Session = "",
    # Verifier vendor to configure; must differ from the builder's vendor.
    [string]$VerifierVendor = "openai"
)

$ErrorActionPreference = "Stop"
function Fail($msg) { Write-Host "refused: $msg" -ForegroundColor Red; exit 1 }
function Assert-True($cond, $msg) { if (-not $cond) { Fail "assertion failed: $msg" } }

# --- the candidate worktree must be clean; only this untracked script is ignored ----------------
$repo = (& git rev-parse --show-toplevel 2>$null)
if (-not $repo) { Fail "run this from inside the candidate repository" }
$self = "release/cross-vendor-receipt.ps1"
$dirty = @(& git -C $repo status --porcelain | Where-Object {
    $_ -ne "" -and -not ($_.Trim() -eq "?? $self") -and -not ($_.Trim() -eq "M $self")
})
if ($dirty.Count -gt 0) { Fail "the candidate worktree is dirty; commit or stash first: $($dirty -join '; ')" }

# --- built binaries (adapter, not a raw CLI) ----------------------------------------------------
$vdt = Join-Path $repo "target\release\vdt.exe"
if (-not (Test-Path $vdt)) { $vdt = Join-Path $repo "target\debug\vdt.exe" }
if (-not (Test-Path $vdt)) { Fail "no built vdt.exe (run cargo build -p verdict-cli)" }
$adapter = Join-Path $repo "target\release\vdt-provider-codex.exe"
if (-not (Test-Path $adapter)) { $adapter = Join-Path $repo "target\debug\vdt-provider-codex.exe" }
if (-not (Test-Path $adapter)) { Fail "no desk adapter (vdt-provider-codex) built; run cargo build -p verdict-desks --bins" }

# --- CLI availability, recorded only; nothing is invoked ----------------------------------------
$codex = (& cmd /C "codex --version" 2>$null); $codexOk = $LASTEXITCODE -eq 0
$claude = (& cmd /C "claude --version" 2>$null); $claudeOk = $LASTEXITCODE -eq 0
if ($VerifierVendor -ne "openai") { Fail "this harness configures the openai verifier (the codex desk adapter); got '$VerifierVendor'" }
if (-not $codexOk) { Fail "the codex CLI is not on PATH, so the openai desk adapter cannot answer" }

# --- isolated copy ------------------------------------------------------------------------------
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$root = Join-Path $env:TEMP "vdt-cross-vendor-$stamp"
$proj = Join-Path $root "project"
$fakeHome = Join-Path $root "home"
$log = Join-Path $root "desk.log"
$receiptPath = Join-Path $root "receipt.txt"
New-Item -ItemType Directory -Path $proj, $fakeHome -Force | Out-Null
& robocopy $repo $proj /E /NFL /NDL /NJH /NJS /XD ".git" "target" "dist" "node_modules" ".verdict" | Out-Null
if ($LASTEXITCODE -ge 8) { Fail "robocopy failed with $LASTEXITCODE" }
New-Item -ItemType Directory -Path (Join-Path $proj ".verdict") -Force | Out-Null
Copy-Item (Join-Path $repo ".verdict\ledger.db") (Join-Path $proj ".verdict\ledger.db")

# --- the session being verified, and its builder vendor (no inference, no skip) ----------------
Push-Location $proj
$sessions = (& $vdt sessions 2>$null | Out-String)
Pop-Location
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
        model        = "gpt-5-codex"
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
Write-Host "  verifier       : desk adapter $adapter -> codex ($VerifierVendor), model gpt-5-codex"
Write-Host "  CLIs present   : codex=$(if ($codexOk) { $codex } else { 'absent' }); claude=$(if ($claudeOk) { $claude } else { 'absent' })"
Write-Host "  desk protocol  : verdict-desk/1 JSON request, answered by the adapter (not a raw CLI call)"
Write-Host "  desks          : 1 ($($desks[0])); providers: 1; verifier_order: $($orderNow -join ', ')"
Write-Host "  model calls    : 1 desk, 1 prompt (one adapter invocation, one verifier call)"
Write-Host "  command        : $command"
Write-Host "  receipt        : $receiptPath (under the temp root; never in the repo)"
Write-Host "  assertions     : exactly 1 desk in the lane, exactly 1 provider for $VerifierVendor whose"
Write-Host "                   command is the adapter, verifier_order contains $VerifierVendor,"
Write-Host "                   builder $builderVendor != verifier $VerifierVendor, and after the run"
Write-Host "                   exactly 1 desk record in the ledger export."
Write-Host ""

if ($DryRun) {
    Write-Host "dry run: all of the above was actually done (copy, policy, assertions); no model was invoked."
    exit 0
}

# --- the one authorised run ---------------------------------------------------------------------
$env:HOME = $fakeHome
$env:USERPROFILE = $fakeHome
$env:VERDICT_STATE_DIR = (Join-Path $root "state")
& $vdt desks --lane standard --session $Session --project $proj 2>&1 | Tee-Object -FilePath $log | Write-Host
if ($LASTEXITCODE -ne 0) { Write-Host "the desk run failed; see $log" -ForegroundColor Red; exit 1 }

# --- receipt from the temp ledger, with the post-run assertion ----------------------------------
Push-Location $proj
$export = (& $vdt ledger export --jsonl 2>$null | Out-String)
Pop-Location
$deskRecords = @([regex]::Matches($export, '"name":"desk:[^"]+"'))
Assert-True ($deskRecords.Count -eq 1) "expected exactly one desk record in the ledger, got $($deskRecords.Count)"
$deskLine = ($export -split "`n" | Where-Object { $_ -match '"name":"desk:' }) | Select-Object -Last 1
$receipt = @(
    "Cross-vendor desk receipt",
    "timestamp: $(Get-Date -Format o)",
    "candidate repo (clean, untouched): $repo",
    "session: $Session",
    "builder vendor: $builderVendor",
    "verifier: desk adapter $adapter -> codex / $VerifierVendor / gpt-5-codex",
    "command: $command",
    "desk log: $log",
    "desk records: $($deskRecords.Count)",
    "desk record: $deskLine"
) -join "`n"
$receipt | Set-Content $receiptPath -NoNewline
Write-Host "wrote $receiptPath"
