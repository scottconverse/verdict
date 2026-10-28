#!/bin/sh
# Verdict's own release gate, for CI and for a person: read the packet's verdict line and fail
# unless it is CLEAR TO SHIP. It never tags, releases or publishes; those stay the human's.
#
#   sh release/dogfood-gate.sh packet.txt
#
# The packet is produced by `vdt export --packet --lane release` after the release lane ran. The
# match is on the verdict line itself, not a substring anywhere in the document.
set -u

packet="${1:-packet.txt}"
if [ ! -f "$packet" ]; then
    echo "::error::no packet at $packet, so the release lane computed nothing" >&2
    exit 1
fi

line=$(grep -E '^VERDICT:' "$packet" | head -n 1 || true)
echo "verdict line: ${line:-<none>}"
if printf '%s' "$line" | grep -Eq '^VERDICT: CLEAR TO SHIP([[:space:]]|$)'; then
    echo "the release lane is CLEAR TO SHIP for this candidate"
    exit 0
fi

echo "::error::the release lane did not compute CLEAR TO SHIP for this candidate; the packet names what is wrong or not established. This gate is expected to be red until the candidate is release-ready."
echo "----- what is proven wrong (FIX FIRST) -----"
sed -n '/^FIX FIRST/,/^CHECKS/p' "$packet" | head -n 60 || true
echo "----- what was not established -----"
sed -n '/^NOT ESTABLISHED/,$p' "$packet" | head -n 40 || true
exit 1
