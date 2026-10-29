# Installed fail-open acceptance — packaged core at `b75010f`

## Independent stream/readback correction, October 29

The original exercise below did not read/assert its redirected output streams or
compute a session verdict. A later affected check used the identical installed
core on a byte-identical copy of that fixture's DB and head, preserving the source
hashes. Writable stdin-open: exit 0 in 451 ms; locked ledger: exit 0 in 1,904 ms.
Both stdout and stderr were explicitly captured and empty. The copy retained the
original record verbatim and added exactly one `check_unavailable`; the original
one-record ledger stayed unchanged. Local raw receipt:
`Documents/Codex/2026-10-28/verdict-installed-stream-acceptance/RECEIPT.json`.

The original fixture's actual `vdt report` output is `no sessions recorded yet`.
Therefore this proves bounded silent installed fail-open and unavailable recording,
not an observed session-level UNVERIFIED verdict or a real host/model interaction.

## Original exercise (historical timings)

Run on this Windows host, isolated root
`C:\Users\scott\AppData\Local\Temp\vdt-installed-acc-20261028-235706` (fresh `home`, `project`,
`state`), from the package built at `b75010f` (zip `9533eacf…3827`, 12802840 bytes).

* Installed core: `C:\…\vdt-installed-acc-20261028-235706\install\vdt.exe`
  SHA-256 **`b1b32ce932dd017ab069c70c7c541d187023aab0a23d7d8ef746f477c410aa46`** — identical to the
  package's inner `vdt.exe` (`b1b32ce9…aa46`, 21232128 bytes). This is the first recorded install of
  the `b75010f` package; the earlier installed retest used the `08ec95c` core
  (`f8c3b18b…ae63`), which stands as history in `CANDIDATE-ARTIFACTS.txt`.
* Server started from the installed core on port **4771** (not 4717), pid **26456**;
  `GET /readyz` returned
  `{"app":"verdict","project":"\\\\?\\C:\\Users\\scott\\AppData\\Local\\Temp\\vdt-installed-acc-20261028-235706\\project","pid":26456}`
  (app id + canonical project + matching pid). The server was then stopped by its own pid.

## Fail-open, writable ledger

`vdt hook codex Stop` from the installed core, `VDT_HOOK_BUDGET_MS=400`, stdin opened and never closed:
exit **0**, elapsed **442 ms**. Stream silence was established only by the later check above.
The project ledger then held **exactly one** matching record:
`check_unavailable` — `"component":"adapter:codex","reason":"Stop: the payload did not arrive within 400 ms, so the event was not read"`.
That is the behaviour the fix targets: writable ledger ⇒ the record lands before the hook exits.

## Fail-open, locked ledger

The ledger file was opened with `FileShare.None` (write lock) and the same hook run: exit **0**,
elapsed **1.897 s** (payload budget 400 ms plus the locked open failing rather than waiting),
**no new record** — the project ledger still had exactly its one earlier record — so the hook
returned fast and failed open. This fixture has no recorded session, so it supplies no
session-verdict observation.

No visible windows were opened, no live settings were edited, the ledger was not reset, no model
calls were made and no release lane was re-run.
