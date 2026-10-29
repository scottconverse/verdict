"""Read-only SQLite backup, including committed WAL, retaining the original head.

This is acceptance tooling, not a ledger repair. Refuse a moving/mismatched
head; never invent a replacement head or overwrite an existing destination.
The caller must also run `vdt ledger verify` on the copy before using it.
"""
import argparse
from contextlib import closing
import hashlib
import json
from pathlib import Path
import sqlite3


def copy_ledger(source: Path, destination: Path):
    source = source.resolve()
    destination = destination.resolve()
    head_path = source.with_name("ledger.head")
    original_head = head_path.read_bytes()
    if destination.exists() or destination.with_name("ledger.head").exists():
        raise ValueError("Destination ledger or head already exists")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as src:
        with closing(sqlite3.connect(destination)) as dst:
            src.backup(dst)
            last = dst.execute("SELECT seq, hash FROM records ORDER BY seq DESC LIMIT 1").fetchone()
            if last is None or original_head.decode().split() != [str(last[0]), last[1]]:
                raise ValueError("Source head does not match the copied ledger tail")
            if head_path.read_bytes() != original_head:
                raise ValueError("Source head changed during backup; do not use this copy")
    destination.with_name("ledger.head").write_bytes(original_head)
    return {"source": str(source), "destination": str(destination),
            "tail_sequence": last[0], "tail_hash": last[1],
            "head_sha256": hashlib.sha256(original_head).hexdigest(),
            "method": "SQLite read-only online backup; original head retained"}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    print(json.dumps(copy_ledger(args.source, args.destination)))
