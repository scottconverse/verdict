import importlib.util
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("copy_ledger", Path(__file__).with_name("copy-ledger.py"))
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class CopyLedgerTest(unittest.TestCase):
    def test_committed_wal_and_head_are_preserved_and_bad_head_is_refused(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            source = root / "ledger.db"
            with closing(sqlite3.connect(source)) as writer:
                writer.execute("PRAGMA journal_mode=WAL")
                writer.execute("CREATE TABLE records(seq INTEGER PRIMARY KEY, hash TEXT)")
                writer.execute("INSERT INTO records VALUES(1, 'old')")
                writer.commit()
                writer.execute("INSERT INTO records VALUES(2, 'new')")
                writer.commit()
                self.assertTrue(source.with_name("ledger.db-wal").exists())
                head = root / "ledger.head"
                head.write_text("2 new\n")
                dest = root / "copy" / "ledger.db"
                receipt = helper.copy_ledger(source, dest)
                self.assertEqual(receipt["tail_sequence"], 2)
                with closing(sqlite3.connect(dest)) as copied:
                    self.assertEqual(copied.execute("SELECT * FROM records").fetchall(), [(1, 'old'), (2, 'new')])
                self.assertEqual(dest.with_name("ledger.head").read_bytes(), head.read_bytes())
                with self.assertRaisesRegex(ValueError, "already exists"):
                    helper.copy_ledger(source, dest)
                head.write_text("3 missing\n")
                with self.assertRaisesRegex(ValueError, "does not match"):
                    helper.copy_ledger(source, root / "bad" / "ledger.db")
                self.assertEqual(head.read_text(), "3 missing\n")
                self.assertFalse((root / "bad" / "ledger.head").exists())


if __name__ == "__main__":
    unittest.main()
