//! A 1,000-file project is read in well under the 30 seconds the design allows, and reading it in
//! parallel gives exactly the facts reading it one file at a time gives.

use std::time::Instant;
use verdict_extract::{ScanOptions, extract_files, extract_tree, list_files, registry};

fn py_file(i: usize) -> String {
    let mut s = String::from(
        "from fastapi import APIRouter, Depends\nfrom sqlalchemy.orm import Session\n\nfrom app.models import Item, Tag\n\n",
    );
    s.push_str(&format!("router = APIRouter(prefix=\"/r{i}\")\n\n"));
    for j in 0..12 {
        s.push_str(&format!(
            "\n@router.post(\"/items/{{item_id}}/part{j}\")\nasync def handler_{i}_{j}(item_id: int, db: Session = Depends(get_db)):\n    \"\"\"Handler {j}.\"\"\"\n    item = Item(name=\"n{j}\")\n    db.add(item)\n    helper_{i}_{j}(db, item)\n    db.commit()\n    return {{\"id\": item_id}}\n\n\ndef helper_{i}_{j}(db, item):\n    for t in range(3):\n        db.add(Tag(label=str(t), item=item))\n    return [x for x in range(10) if x % 2]\n\n\nclass Thing{i}_{j}:\n    def method(self):\n        raise NotImplementedError\n"
        ));
    }
    s
}

fn ts_file(i: usize) -> String {
    let mut s = String::from("import { useState } from \"react\";\n\n");
    for j in 0..12 {
        s.push_str(&format!(
            "export function Page{i}_{j}() {{\n  const [v, setV] = useState(0);\n  async function save{j}() {{\n    await fetch(`/api/r{i}/items/${{v}}/part{j}`, {{ method: \"POST\" }});\n    setV(v + 1);\n  }}\n  return <button onClick={{save{j}}}>Save {j}</button>;\n}}\n\n"
        ));
    }
    s
}

fn write_project(dir: &std::path::Path, files: usize) {
    for i in 0..files {
        let (path, text) = if i % 2 == 0 {
            (format!("backend/app/mod_{i}.py"), py_file(i))
        } else {
            (format!("web/src/page_{i}.tsx"), ts_file(i))
        };
        let full = dir.join(&path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
}

#[test]
fn a_thousand_mixed_files_extract_in_under_thirty_seconds() {
    let dir = tempfile::tempdir().unwrap();
    write_project(dir.path(), 1000);
    let start = Instant::now();
    let facts = extract_tree(dir.path(), &ScanOptions::default());
    let took = start.elapsed();
    println!(
        "extract_tree: 1000 files (500 Python, 500 TSX) in {:.2?}; {} routes, {} writes, {} definitions",
        took,
        facts.endpoints().count(),
        facts.writes().count(),
        facts.defs().count()
    );
    assert_eq!(facts.files.len(), 1000);
    assert!(facts.is_complete());
    assert_eq!(facts.endpoints().count(), 500 * 12);
    assert!(
        took.as_secs() < 30,
        "1,000 files took {took:?}, the limit is 30 seconds"
    );
}

#[test]
fn parallel_and_serial_extraction_give_identical_facts() {
    let dir = tempfile::tempdir().unwrap();
    write_project(dir.path(), 120);
    let opts = ScanOptions::default();
    let (paths, _, _) = list_files(dir.path(), &opts);
    let all = extract_files(dir.path(), &paths, &opts, &registry());
    assert_eq!(all.files.len(), 120);
    // One file at a time (a single path never spawns threads): every file's facts are the same
    // as in the parallel run. (Routers here are never mounted, so the link pass adds nothing
    // across files.)
    for f in &all.files {
        let one = extract_files(
            dir.path(),
            std::slice::from_ref(&f.path),
            &opts,
            &registry(),
        );
        assert_eq!(one.files.len(), 1);
        assert_eq!(&one.files[0], f, "{}", f.path);
    }
    // And a second parallel run gives the same bytes.
    let again = extract_files(dir.path(), &paths, &opts, &registry());
    assert_eq!(all, again, "same input, same facts");
    let tree = extract_tree(dir.path(), &opts);
    assert_eq!(tree.files, all.files);
}
