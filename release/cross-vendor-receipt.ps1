# SPEC-8 cross-vendor receipt harness
#
# Prepares (and, without -DryRun, runs) exactly ONE verifier desk from a vendor different from the
# builder, against a copy of the candidate project, and writes a receipt with the served identity
# fields. Nothing here touches `merge-trial` itself: the work happens in a fresh temp copy with its
# own ledger and its own HOME.
#
#   pwsh -File release/cross-vendor-receipt.ps1 -DryRun          # shows the plan, invokes no model
#   pwsh -File release/cross-vendor-receipt.ps1                  # the one authorised model call
#
# Authorising this authorises exactly one desk call: one prompt to the configured verifier CLI.

param(
    # Perform every step except the desk (model) invocation.
    [switch]$DryRun,
    # Verifier vendor to configure. Must differ from the builder's vendor.
    [string]$VerifierVendor = "openai",
    # Verifier CLI to use; detected automatically when omitted.
    [string]$VerifierCommand = ""
)

$ErrorActionPreference = "Stop"

function Fail($msg) { Write-Host "refused: $msg" -ForegroundColor Red; exit 1 }

# --- the candidate worktree must be clean -------------------------------------------------------
$repo = (& git rev-parse --show-toplevel 2>$null)
if (-not $repo) { Fail "run this from inside the candidate repository" }
$self = "release/cross-vendor-receipt.ps1"
$dirty = @(& git -C $repo status --porcelain | Where-Object { $_ -ne "" -and $_ -notmatch [regex]::Escape($self) })
if ($dirty.Count -gt 0) { Fail "the candidate worktree is dirty; commit or stash first: $($dirty -join '; ')" }

# --- which CLIs are actually present (availability only; nothing is run) -----------------------
$codex = (& cmd /C "codex --version" 2>$null); $codexOk = $LASTEXITCODE -eq 0
$claude = (& cmd /C "claude --version" 2>$null); $claudeOk = $LASTEXITCODE -eq 0
if (-not $VerifierCommand) {
    $VerifierCommand = if ($VerifierVendor -eq "openai") { "codex" } elseif ($VerifierVendor -eq "anthropic") { "claude" } else { "" }
}
if (-not $VerifierCommand) { Fail "no CLI known for vendor '$VerifierVendor' (codex = openai, claude = anthropic)" }
if ($VerifierCommand -eq "codex" -and -not $codexOk) { Fail "codex is not on PATH" }
if ($VerifierCommand -eq "claude" -and -not $claudeOk) { Fail "claude is not on PATH" }

# --- the builder's vendor, so the verifier really is cross-vendor -------------------------------
$vdt = Join-Path $repo "target\release\vdt.exe"
if (-not (Test-Path $vdt)) { $vdt = Join-Path $repo "target\debug\vdt.exe" }
if (-not (Test-Path $vdt)) { Fail "no built vdt.exe (run cargo build -p verdict-cli)" }
$sessions = (& $vdt sessions 2>$null | Out-String)
$builderVendor = if ($sessions -match "vendor\s+(\w+)") { $Matches[1] } else { "" }
if ($builderVendor -and $builderVendor -ieq $VerifierVendor) {
    Fail "verifier vendor '$VerifierVendor' equals the builder's vendor '$builderVendor'; cross-vendor needs a different one"
}

# --- the isolated copy --------------------------------------------------------------------------
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$root = Join-Path $env:TEMP "vdt-cross-vendor-$stamp"
$proj = Join-Path $root "project"
$fakeHome = Join-Path $root "home"
$log = Join-Path $root "desk.log"
New-Item -ItemType Directory -Path $proj, $fakeHome -Force | Out-Null
& robocopy $repo $proj /E /NFL /NDL /NJH /NJS /XD ".git" "target" "dist" "node_modules" ".verdict" | Out-Null
if ($LASTEXITCODE -ge 8) { Fail "robocopy failed with $LASTEXITCODE" }
# Its own ledger and policy, copied from the candidate so the session exists; never shared.
New-Item -ItemType Directory -Path (Join-Path $proj ".verdict") -Force | Out-Null
Copy-Item (Join-Path $repo ".verdict\ledger.db") (Join-Path $proj ".verdict\ledger.db")

# --- configure exactly one verifier and exactly one desk ------------------------------------------
$policyPath = Join-Path $proj "verdict.json"
$policy = Get-Content $policyPath -Raw | ConvertFrom-Json
$policy.vendors.providers = @(
    [pscustomobject]@{
        name         = "cross-vendor-$VerifierCommand"
        vendor       = $VerifierVendor
        command      = $VerifierCommand
        args         = @("exec")
        model        = if ($VerifierVendor -eq "openai") { "gpt-5-codex" } else { "claude-sonnet-4-5" }
        timeout_secs = 300
    }
)
# One desk only, so the authorised cost is a single prompt to a single CLI.
$policy.lanes.standard.desks = @($policy.lanes.standard.desks[0])
($policy | ConvertTo-Json -Depth 30) | Set-Content $policyPath -NoNewline

$command = "$vdt desks --lane standard --project `"$proj`""

Write-Host ""
Write-Host "Cross-vendor desk receipt — plan"
Write-Host "  candidate repo : $repo (clean; untouched)"
Write-Host "  temp copy      : $proj (its own ledger; HOME=$fakeHome)"
Write-Host "  builder vendor : $(if ($builderVendor) { $builderVendor } else { 'unknown' })"
Write-Host "  verifier       : $VerifierCommand ($VerifierVendor), model $(($policy.vendors.providers[0]).model), args exec"
Write-Host "  CLIs present   : codex=$(if ($codexOk) { $codex } else { 'absent' }); claude=$(if ($claudeOk) { $claude } else { 'absent' })"
Write-Host "  desks          : exactly 1 ($(($policy.lanes.standard.desks | Select-Object -First 1)))"
Write-Host "  model calls    : 1 desk, 1 prompt (one CLI invocation). No other model calls are made."
Write-Host "  command        : $command"
Write-Host "  receipt        : release/cross-vendor-receipt-$stamp.txt (written from the temp ledger)"
Write-Host ""

if ($DryRun) {
    Write-Host "dry run: nothing was invoked; the command above is exactly what an authorised run executes."
    exit 0
}

# --- the one authorised run ---------------------------------------------------------------------
$env:HOME = $fakeHome
$env:USERPROFILE = $fakeHome
$env:VERDICT_STATE_DIR = (Join-Path $root "state")
& $vdt desks --lane standard --project $proj 2>&1 | Tee-Object -FilePath $log | Write-Host
if ($LASTEXITCODE -ne 0) { Write-Host "the desk run failed; see $log" -ForegroundColor Red; exit 1 }

# --- receipt from the temp ledger ---------------------------------------------------------------
Push-Location $proj
$export = & $vdt ledger export --jsonl 2>$null | Out-String
Pop-Location
$deskLines = ($export -split "`n" | Where-Object { $_ -match '"name":"desk:' }) | Select-Object -Last 1
$receipt = @(
    "Cross-vendor desk receipt",
    "timestamp: $(Get-Date -Format o)",
    "candidate repo (clean, untouched): $repo",
    "temp project: $proj",
    "builder vendor: $builderVendor",
    "verifier: $VerifierCommand / $VerifierVendor / model $(($policy.vendors.providers[0]).model)",
    "command: $command",
    "desk log: $log",
    "desk record: $deskLines"
) -join "`n"
$receiptPath = Join-Path $repo "release\cross-vendor-receipt-$stamp.txt"
$receipt | Set-Content $receiptPath -NoNewline
Write-Host "wrote $receiptPath"
