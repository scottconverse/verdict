mod common;

use common::*;
use std::process::Command;
use verdict_core::*;

fn git(dir: &std::path::Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn params(home: &std::path::Path) -> SessionParams {
    SessionParams {
        agent: "claude".into(),
        model: "m".into(),
        vendor: "anthropic".into(),
        home: Some(home.to_path_buf()),
    }
}

#[test]
fn baseline_in_a_non_git_folder_says_so() {
    let t = TempDir::new("nogit");
    std::fs::write(t.path().join("a.txt"), "x").unwrap();
    let b = baseline_snapshot(t.path());
    assert!(!b.is_git_repo);
    assert!(b.git.is_none());
    assert!(b.note.is_some());
    assert_eq!(b.hash.len(), 64);
}

#[test]
fn baseline_in_a_git_repo_tracks_tree_and_uncommitted_content() {
    let t = TempDir::new("git");
    let p = t.path();
    if !git(p, &["init", "-q"]) {
        eprintln!("git not available; skipping");
        return;
    }
    std::fs::write(p.join("a.txt"), "one\n").unwrap();
    assert!(git(p, &["add", "."]) && git(p, &["commit", "-q", "-m", "c1"]));
    let clean = baseline_snapshot(p);
    assert!(clean.is_git_repo);
    let g = clean.git.clone().unwrap();
    assert!(g.tree_hash.is_some() && g.head_commit.is_some());
    assert_eq!(g.uncommitted_files, 0);
    assert_eq!(baseline_snapshot(p), clean, "stable when nothing changed");

    std::fs::write(p.join("a.txt"), "two\n").unwrap();
    let dirty = baseline_snapshot(p);
    assert_ne!(dirty.hash, clean.hash);
    assert_eq!(dirty.git.as_ref().unwrap().tree_hash, g.tree_hash);
    assert_eq!(dirty.git.as_ref().unwrap().uncommitted_files, 1);

    std::fs::write(p.join("a.txt"), "three\n").unwrap();
    assert_ne!(
        baseline_snapshot(p).hash,
        dirty.hash,
        "content, not just names"
    );

    std::fs::write(p.join("new.txt"), "n").unwrap();
    assert_eq!(baseline_snapshot(p).git.unwrap().uncommitted_files, 2);
    // The ledger folder never counts.
    std::fs::create_dir_all(p.join(".verdict")).unwrap();
    std::fs::write(p.join(".verdict").join("x"), "n").unwrap();
    assert_eq!(baseline_snapshot(p).git.unwrap().uncommitted_files, 2);
}

#[test]
fn baseline_in_a_repo_with_no_commits() {
    let t = TempDir::new("empty");
    if !git(t.path(), &["init", "-q"]) {
        return;
    }
    let b = baseline_snapshot(t.path());
    assert!(b.is_git_repo);
    assert_eq!(b.git.unwrap().tree_hash, None);
}

#[test]
fn session_record_holds_baseline_agent_model_vendor_policy_version() {
    let t = TempDir::new("sess");
    let home = TempDir::new("home");
    let mut l = Ledger::open(t.path()).unwrap();
    let s = start_session(&mut l, t.path(), &params(home.path())).unwrap();
    assert!(s.policy_warning.is_none());
    let rec = l.get(s.seq).unwrap().unwrap();
    assert_eq!(rec.writer, Writer::Adapter);
    let Record::Session {
        baseline,
        agent,
        model,
        vendor,
        policy_version,
        started_at,
        ..
    } = rec.record
    else {
        panic!()
    };
    assert_eq!(
        (agent.as_str(), model.as_str(), vendor.as_str()),
        ("claude", "m", "anthropic")
    );
    assert_eq!(policy_version, 1);
    assert!(!started_at.is_empty());
    assert_eq!(baseline.hash, s.baseline.hash);
    let sessions = list_sessions(&l).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, s.id);
}

#[test]
fn missing_policy_files_fall_back_to_the_embedded_default() {
    let t = TempDir::new("pol0");
    let home = TempDir::new("polhome0");
    let p = Policy::load(t.path(), Some(home.path())).unwrap();
    assert_eq!(p.sources, vec![PolicySource::BuiltIn]);
    assert_eq!(p, Policy::builtin().unwrap());
    assert_eq!(p.version, 1);
    assert_eq!(p.gate_mode(), "shadow");
}

#[test]
fn project_policy_goes_on_top_of_global_on_top_of_default() {
    let t = TempDir::new("pol1");
    let home = TempDir::new("polhome1");
    std::fs::create_dir_all(home.path().join(".verdict")).unwrap();
    std::fs::write(
        home.path().join(".verdict").join("policy.json"),
        r#"{"version":1,"gate":{"max_sendbacks":5},"exclude":["global"]}"#,
    )
    .unwrap();
    std::fs::write(
        t.path().join("verdict.json"),
        r#"{"version":1,"gate":{"mode":"enforce"},"exclude":["project"]}"#,
    )
    .unwrap();
    let p = Policy::load(t.path(), Some(home.path())).unwrap();
    assert_eq!(p.sources.len(), 3);
    assert_eq!(p.gate_mode(), "enforce");
    assert_eq!(p.value["gate"]["max_sendbacks"], 5, "global layer survives");
    assert_eq!(p.value["gate"]["stop_timeout"], "20s", "default survives");
    assert_eq!(p.value["exclude"], serde_json::json!(["project"]));
    assert_ne!(p.hash, Policy::builtin().unwrap().hash);
}

#[test]
fn invalid_policy_is_rejected_by_the_schema_and_the_session_fails_open() {
    let t = TempDir::new("pol2");
    let home = TempDir::new("polhome2");
    std::fs::write(
        t.path().join("verdict.json"),
        r#"{"version":1,"gate":{"mode":"sometimes"}}"#,
    )
    .unwrap();
    let err = Policy::load(t.path(), Some(home.path())).unwrap_err();
    assert!(matches!(err, PolicyError::Invalid(..)), "{err}");
    assert!(err.to_string().contains("verdict.json"));

    // Not a Verdict policy at all (wrong version, wrong type).
    std::fs::write(t.path().join("verdict.json"), r#"{"version":2}"#).unwrap();
    assert!(Policy::load(t.path(), Some(home.path())).is_err());
    std::fs::write(t.path().join("verdict.json"), "not json").unwrap();
    assert!(matches!(
        Policy::load(t.path(), Some(home.path())),
        Err(PolicyError::Json(..))
    ));

    let mut l = Ledger::open(t.path()).unwrap();
    let s = start_session(&mut l, t.path(), &params(home.path())).unwrap();
    assert!(s.policy_warning.is_some());
    assert_eq!(s.policy_version, 1);
    let kinds: Vec<_> = l.records().unwrap().iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![RecordKind::Session, RecordKind::CheckUnavailable]
    );
}

#[test]
fn evidence_pointers_detect_stale_lines_and_assets() {
    let t = TempDir::new("ptr");
    let p = t.path();
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(p.join("src").join("a.rs"), "fn a() {}\r\nfn b() {}\r\n").unwrap();
    std::fs::write(p.join("rel.zip"), b"bytes").unwrap();

    let line = EvidencePointer::file_line(p, "src\\a.rs", 2).unwrap();
    let asset = EvidencePointer::asset(p, "rel.zip").unwrap();
    let cmd = EvidencePointer::command("cargo test", b"ok");
    assert_eq!(line.describe(), "src/a.rs:2");
    assert_eq!(line.check(p), PointerStatus::Current);
    assert_eq!(asset.check(p), PointerStatus::Current);
    assert_eq!(cmd.check(p), PointerStatus::NotCheckable);
    assert!(EvidencePointer::file_line(p, "src/a.rs", 9).is_err());

    std::fs::write(p.join("src").join("a.rs"), "fn a() {}\nfn b() { 1 }\n").unwrap();
    assert!(matches!(line.check(p), PointerStatus::Stale(_)));
    std::fs::write(p.join("src").join("a.rs"), "fn a() {}\n").unwrap();
    assert!(matches!(line.check(p), PointerStatus::Stale(_)));
    std::fs::write(p.join("rel.zip"), b"other").unwrap();
    assert!(matches!(asset.check(p), PointerStatus::Stale(_)));
    std::fs::remove_file(p.join("rel.zip")).unwrap();
    assert!(matches!(asset.check(p), PointerStatus::Stale(_)));
}

#[test]
fn proof_tiers_serialize_as_the_seven_documented_names() {
    let all = [
        Tier::Static,
        Tier::Ran,
        Tier::Browsed,
        Tier::Asset,
        Tier::Judgment,
        Tier::HonestRed,
        Tier::Unverified,
    ];
    let names: Vec<String> = all
        .iter()
        .map(|t| {
            serde_json::to_value(t)
                .unwrap()
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        [
            "static",
            "ran",
            "browsed",
            "asset",
            "judgment",
            "honest_red",
            "unverified"
        ]
    );
    for (t, n) in all.iter().zip(&names) {
        assert_eq!(t.as_str(), n);
    }
}

#[test]
fn project_root_is_found_from_a_subfolder() {
    let t = TempDir::new("root");
    let sub = t.path().join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    drop(Ledger::open(t.path()).unwrap());
    let found = find_project_root(&sub);
    assert_eq!(
        found.canonicalize().unwrap(),
        t.path().canonicalize().unwrap()
    );
}
