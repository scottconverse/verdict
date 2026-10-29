$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'ledger-receipt-delta.ps1')
function Row($seq, $session = 's-target', $name = 'desk:engineer', $outcome = 'ran') {
    [pscustomobject][ordered]@{seq=$seq;session_id=$session;kind='check';body=[pscustomobject]@{name=$name;outcome=$outcome};hash="hash-$seq"}
}
function Refuses([scriptblock]$Action, [string]$Pattern) {
    try { & $Action | Out-Null } catch { if ($_.Exception.Message -notmatch $Pattern) { throw }; return }
    throw "Expected rejection: $Pattern"
}
$before = @(1..76 | ForEach-Object { Row $_ })
$after = @($before) + @(Row 77)
$oldCount = [regex]::Matches(($after | ForEach-Object { $_ | ConvertTo-Json -Depth 10 -Compress }) -join "`n", '"name":"desk:[^"]+"').Count
if ($oldCount -ne 77) { throw 'Failed to reproduce historical total-count defect' }
$jsonl = ($after | ForEach-Object { $_ | ConvertTo-Json -Depth 10 -Compress }) -join "`n"
$parsed = @(Read-LedgerReceiptRows $jsonl)
$result = Assert-AppendOnlyDeskReceipt $before $parsed 's-target' 'engineer'
if ($result.seq -ne 77 -or $before.Count -ne 76) { throw 'Wrong delta or altered history' }
Refuses { Assert-AppendOnlyDeskReceipt $before $before 's-target' 'engineer' } 'NEW desk'
Refuses { Assert-AppendOnlyDeskReceipt $before ($after + @(Row 78)) 's-target' 'engineer' } 'NEW desk'
Refuses { Assert-AppendOnlyDeskReceipt $before (@($before) + @(Row 77 's-other')) 's-target' 'engineer' } 'session and desk'
Refuses { Assert-AppendOnlyDeskReceipt $before (@($before) + @(Row 77 's-target' 'desk:security')) 's-target' 'engineer' } 'session and desk'
Refuses { Assert-AppendOnlyDeskReceipt $before (@($before) + @(Row 77 's-target' 'desk:engineer' 'failed_to_run')) 's-target' 'engineer' } 'successfully'
Refuses { Assert-AppendOnlyDeskReceipt $before @() 's-target' 'engineer' } 'removed'
$changed = @($before | ConvertTo-Json -Depth 10 | ConvertFrom-Json); $changed[0].hash = 'changed'
Refuses { Assert-AppendOnlyDeskReceipt $before ($changed + @(Row 77)) 's-target' 'engineer' } 'history changed'
Refuses { Read-LedgerReceiptRows '{bad-json' } 'JSON'
Refuses { Read-LedgerReceiptRows '{"seq":1,"hash":"a"}
{"seq":1,"hash":"b"}' } 'increasing'
Write-Output 'PASS: historical 77-total defect reproduced; new receipt selected; nine rejection cases preserved. Synthetic fixture only, no model or real ledger execution.'
