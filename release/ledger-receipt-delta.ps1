# Isolated candidate helper. No database mutation or external execution.
function Read-LedgerReceiptRows([string]$JsonLines) {
    $lastSeq = 0L
    foreach ($line in ($JsonLines -split "`r?`n")) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $row = $line | ConvertFrom-Json -ErrorAction Stop
        if ($null -eq $row.seq -or $row.seq -le $lastSeq -or $row.seq -ne [long]$row.seq -or -not $row.hash) {
            throw 'Ledger export must contain increasing positive sequence numbers and hashes'
        }
        $lastSeq = [long]$row.seq
        $row
    }
}

function Assert-AppendOnlyDeskReceipt([array]$Before, [array]$After, [string]$Session, [string]$Desk) {
    if ($After.Count -lt $Before.Count) { throw 'Copied ledger history was removed' }
    for ($i = 0; $i -lt $Before.Count; $i++) {
        # Compare exported records, not merely caller-supplied hashes. No reset is allowed.
        if (($Before[$i] | ConvertTo-Json -Depth 100 -Compress) -cne ($After[$i] | ConvertTo-Json -Depth 100 -Compress)) {
            throw "Copied ledger history changed at index $i"
        }
    }
    $newDesks = @($After | Select-Object -Skip $Before.Count | Where-Object { $_.kind -eq 'check' -and $_.body.name -like 'desk:*' })
    if ($newDesks.Count -ne 1) { throw "Expected exactly one NEW desk receipt, got $($newDesks.Count)" }
    $receipt = $newDesks[0]
    if ($receipt.session_id -cne $Session -or $receipt.body.name -cne "desk:$Desk") { throw 'New desk receipt does not match the requested session and desk' }
    if ($receipt.body.outcome -cne 'ran') { throw 'New desk receipt did not run successfully' }
    return $receipt
}
