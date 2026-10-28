mod p2b_common;

use p2b_common::*;
use verdict_core::lane::*;
use verdict_core::verdict::{compute_live, live_input};
use verdict_core::*;

/// An explicit project policy, the way a real project supplies its blast globs. The built-in
/// default has none (see the last test), so the lane tests configure what counts as high and
/// release rather than relying on any application's paths.
fn policy() -> Policy {
    let files = [(
        "verdict.json",
        r#"{"version":1,"blast":{
                "high_if_touches":["**/auth/**","**/migrations/**","**/routes/**","package.json"],
                "release_if_touches":["changelog.md","**/installer/**"]
            }}"#,
    )];
    let dir = p2b_common::Tmp::new("lane-policy");
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body).unwrap();
    }
    let home = p2b_common::Tmp::new("lane-home");
    Policy::load(dir.path(), Some(home.path())).unwrap()
}

fn paths(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_lane_comes_from_what_the_diff_touches() {
    let p = policy();
    assert_eq!(classify_lane(&p, &[]).lane, Lane::Low);
    assert_eq!(
        classify_lane(&p, &paths(&["README.md", "docs/a.md", "logo.png"])).lane,
        Lane::Low
    );
    assert_eq!(
        classify_lane(&p, &paths(&["src/util.ts"])).lane,
        Lane::Standard
    );
    assert_eq!(
        classify_lane(&p, &paths(&["src/routes/home.ts"])).lane,
        Lane::High
    );
    assert_eq!(
        classify_lane(&p, &paths(&["src/server/auth/login.ts"])).lane,
        Lane::High
    );
    assert_eq!(
        classify_lane(&p, &paths(&["migrations/001_init.sql"])).lane,
        Lane::High
    );
    assert_eq!(
        classify_lane(&p, &paths(&["package.json"])).lane,
        Lane::High
    );
    assert_eq!(
        classify_lane(&p, &paths(&["CHANGELOG.md"])).lane,
        Lane::Release
    );
    assert_eq!(
        classify_lane(&p, &paths(&["installer/setup.iss"])).lane,
        Lane::Release
    );
    // The highest thing touched wins.
    assert_eq!(
        classify_lane(
            &p,
            &paths(&[
                "README.md",
                "src/util.ts",
                "src/routes/a.ts",
                "CHANGELOG.md"
            ])
        )
        .lane,
        Lane::Release
    );
}

#[test]
fn diff_size_never_matters() {
    let p = policy();
    // Five hundred documentation files are still low; one auth file is high.
    let many: Vec<String> = (0..500).map(|i| format!("docs/page{i}.md")).collect();
    assert_eq!(classify_lane(&p, &many).lane, Lane::Low);
    assert_eq!(
        classify_lane(&p, &paths(&["src/server/auth/session.ts"])).lane,
        Lane::High
    );
    // Many code files without a blast match are standard, not high.
    let code: Vec<String> = (0..500).map(|i| format!("src/m{i}.ts")).collect();
    assert_eq!(classify_lane(&p, &code).lane, Lane::Standard);
}

#[test]
fn a_glob_without_a_slash_matches_at_any_depth_and_case_does_not_lower_a_lane() {
    let p = policy();
    assert_eq!(
        classify_lane(&p, &paths(&["web/app/package.json"])).lane,
        Lane::High
    );
    assert_eq!(
        classify_lane(&p, &paths(&["changelog.md"])).lane,
        Lane::Release
    );
    assert_eq!(
        classify_lane(&p, &paths(&["SRC/Routes/a.ts"])).lane,
        Lane::High
    );
}

#[test]
fn excluded_and_tool_folders_are_not_part_of_the_diff() {
    let p = policy();
    let root = std::path::Path::new("/work/proj");
    let n = normalize_paths(
        &p,
        root,
        &paths(&[
            "./src/a.ts",
            "/work/proj/src/b.ts",
            ".verdict/ledger.db",
            "generated/api.ts",
            "pkg/vendor/lib.js",
            "src\\win\\c.ts",
        ]),
    );
    assert_eq!(n, vec!["src/a.ts", "src/b.ts", "src/win/c.ts"]);
}

#[test]
fn an_invalid_glob_is_reported_not_silently_trusted() {
    let t = Tmp::new("badglob");
    std::fs::write(
        t.path().join("verdict.json"),
        r#"{"version":1,"blast":{"high_if_touches":["src/[broken"]}}"#,
    )
    .unwrap();
    let home = Tmp::new("home");
    let p = Policy::load(t.path(), Some(home.path())).unwrap();
    let c = classify_lane(&p, &paths(&["src/x.ts"]));
    assert_eq!(c.lane, Lane::Standard);
    assert!(!c.warnings.is_empty(), "{c:?}");
}

#[test]
fn the_builtin_default_has_no_project_paths_and_says_classification_is_missing() {
    let p = Policy::builtin().unwrap();
    // The default policy carries no application's commands, plan file, blast globs, verifier or
    // project paths: a project supplies those explicitly.
    for pointer in [
        "/plans/0",
        "/exec/commands/0",
        "/blast/high_if_touches/0",
        "/blast/release_if_touches/0",
        "/vendors/verifier_order/0",
    ] {
        assert!(
            p.value.pointer(pointer).is_none(),
            "the built-in default still names {pointer}"
        );
    }
    assert!(
        p.value
            .pointer("/exec/commands")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        p.value
            .pointer("/blast/high_if_touches")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Without configuration a code change is never low, and the missing classification is stated
    // rather than silently assumed.
    let c = classify_lane(&p, &paths(&["src/server/auth/login.ts"]));
    assert_eq!(c.lane, Lane::Standard);
    assert!(
        c.reasons
            .iter()
            .any(|r| r.contains("no blast classification is configured")),
        "{c:?}"
    );
    assert!(
        c.warnings
            .iter()
            .any(|w| w.contains("no blast classification"))
    );
    // Documentation only is still low: that classification needs no project knowledge.
    assert_eq!(classify_lane(&p, &paths(&["docs/a.md"])).lane, Lane::Low);
}

#[test]
fn a_raise_lane_decision_raises_and_no_decision_lowers() {
    let t = Tmp::new("raise");
    let mut l = seeded(t.path());
    assert_eq!(
        effective_lane(&l, SID, Lane::Standard).unwrap(),
        Lane::Standard
    );
    human(&mut l, DecisionKind::RaiseLane, "high", None);
    assert_eq!(effective_lane(&l, SID, Lane::Standard).unwrap(), Lane::High);
    // A "raise" below what the diff implies changes nothing.
    human(&mut l, DecisionKind::RaiseLane, "low", None);
    assert_eq!(effective_lane(&l, SID, Lane::Standard).unwrap(), Lane::High);
    assert_eq!(
        effective_lane(&l, SID, Lane::Release).unwrap(),
        Lane::Release
    );
    human(&mut l, DecisionKind::RaiseLane, "release", None);
    assert_eq!(effective_lane(&l, SID, Lane::Low).unwrap(), Lane::Release);
    // A raise in another session does not apply.
    assert_eq!(effective_lane(&l, "s-other", Lane::Low).unwrap(), Lane::Low);
    // Only the human can raise.
    let r = l.append(
        Writer::Agent,
        Some(SID),
        Record::Decision {
            decision: DecisionKind::RaiseLane,
            choice: "release".into(),
            reason: None,
            who: Writer::Human,
        },
    );
    assert!(matches!(r, Err(LedgerError::Refused { .. })));
}

#[test]
fn a_raised_lane_changes_which_checkers_are_required() {
    let t = Tmp::new("raisereq");
    let mut l = seeded(t.path());
    run_all_required(&mut l, Lane::Low);
    assert_eq!(verdict_of(&l, Lane::Low).word, VerdictWord::Verified);
    human(&mut l, DecisionKind::RaiseLane, "standard", None);
    let c = verdict_of(&l, Lane::Low);
    assert_eq!(c.lane, Lane::Standard);
    assert_eq!(c.implied_lane, Lane::Low);
    assert_eq!(
        c.word,
        VerdictWord::Partial,
        "standard also requires exec and the rest"
    );
}

#[test]
fn the_lanes_checker_list_expands_star_and_minus() {
    let p = policy();
    assert_eq!(
        lane_checkers(&p, Lane::Low),
        vec!["wiring", "regressions", "claims_vs_code", "unfinished"]
    );
    assert_eq!(lane_checkers(&p, Lane::Standard).len(), 7);
    let high = lane_checkers(&p, Lane::High);
    assert!(high.contains(&"mutation".to_string()));
    assert!(!high.contains(&"firstrun".to_string()) && !high.contains(&"release".to_string()));
    assert_eq!(
        lane_checkers(&p, Lane::Release),
        ALL_CHECKERS.map(String::from).to_vec()
    );
}

/// The project policy the lane tests use, written into a repository the way a real project would
/// supply its blast globs.
const BLAST_POLICY: &str = r#"{"version":1,"blast":{
  "high_if_touches":["**/auth/**","**/migrations/**","**/routes/**","package.json"],
  "release_if_touches":["changelog.md","**/installer/**"]
}}"#;

fn configure_blast(root: &std::path::Path) {
    std::fs::write(root.join("verdict.json"), BLAST_POLICY).unwrap();
    git(root, &["add", "verdict.json"]);
    git(root, &["commit", "-q", "-m", "policy"]);
}

#[test]
fn live_lane_is_read_from_edit_tool_calls_and_a_real_git_diff() {
    let repo = git_repo("livelane");
    configure_blast(repo.path());
    let home = Tmp::new("home");
    let (mut l, sid) = repo_ledger(repo.path(), home.path());
    // Nothing changed yet: low.
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert_eq!(c.implied_lane, Lane::Low);

    // An edit tool call that names a route file raises it, with no file on disk to prove it.
    l.append(
        Writer::Adapter,
        Some(&sid),
        Record::ToolCall {
            tool: "edit".into(),
            target: "src/routes/home.ts".into(),
            output_hash: "0".repeat(64),
            duration_ms: 1,
        },
    )
    .unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert_eq!(c.implied_lane, Lane::High);
    assert!(
        c.lane_reasons
            .iter()
            .any(|r| r.contains("src/routes/home.ts"))
    );
}

#[test]
fn a_shell_command_is_not_an_edit_and_names_no_path() {
    let repo = git_repo("shell");
    let home = Tmp::new("home");
    let (mut l, sid) = repo_ledger(repo.path(), home.path());
    l.append(
        Writer::Adapter,
        Some(&sid),
        Record::ToolCall {
            tool: "shell".into(),
            target: "rm -rf migrations/x".into(),
            output_hash: "0".repeat(64),
            duration_ms: 1,
        },
    )
    .unwrap();
    let c = compute_live(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert_eq!(c.implied_lane, Lane::Low);
}

#[test]
fn a_changed_file_no_tool_call_mentions_is_found_by_git_unstaged_and_staged() {
    let repo = git_repo("gitfound");
    configure_blast(repo.path());
    let home = Tmp::new("home");
    let (l, sid) = repo_ledger(repo.path(), home.path());
    std::fs::create_dir_all(repo.path().join("migrations")).unwrap();
    std::fs::write(repo.path().join("migrations/001.sql"), "select 1;\n").unwrap();
    let live = live_input(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert_eq!(live.input.implied_lane, Lane::High);
    assert!(
        live.paths.contains(&"migrations/001.sql".to_string()),
        "{:?}",
        live.paths
    );
    git(repo.path(), &["add", "migrations/001.sql"]);
    let live = live_input(repo.path(), &l, &sid, Some(home.path())).unwrap();
    assert_eq!(live.input.implied_lane, Lane::High);
    // The tool's own folder is never part of the diff, even when git does not ignore it.
    assert!(live.paths.iter().all(|p| !p.starts_with(".verdict")));
}

#[test]
fn staged_tree_is_the_identity_of_exactly_what_is_staged() {
    let repo = git_repo("tree");
    let t0 = staged_tree(repo.path()).unwrap();
    std::fs::write(repo.path().join("a.txt"), "one\n").unwrap();
    git(repo.path(), &["add", "a.txt"]);
    let t1 = staged_tree(repo.path()).unwrap();
    assert_ne!(t0, t1);
    assert_eq!(
        staged_tree(repo.path()).unwrap(),
        t1,
        "stable while nothing changes"
    );
    std::fs::write(repo.path().join("a.txt"), "two\n").unwrap();
    git(repo.path(), &["add", "a.txt"]);
    assert_ne!(staged_tree(repo.path()).unwrap(), t1);
    assert_eq!(staged_paths(repo.path()), vec!["a.txt"]);
    // Outside a repository there is no tree.
    let plain = Tmp::new("plain");
    assert!(staged_tree(plain.path()).is_none());
    assert!(staged_paths(plain.path()).is_empty());
}

#[test]
fn a_policy_declared_blockers_only_bar_is_read_as_blockers_only() {
    // The parser is part of the verdict: a project that declares the looser bar must get it, and
    // the stricter default must still apply when the key is absent or unknown.
    let mut p = policy();
    p.value["lanes"]["release"]["bar"] = serde_json::json!("blockers_only");
    assert_eq!(lane_bar(&p, Lane::Release), Bar::BlockersOnly);
    p.value["lanes"]["release"]["bar"] = serde_json::json!("zero_above_nit");
    assert_eq!(lane_bar(&p, Lane::Release), Bar::ZeroAboveNit);
    p.value["lanes"]["release"]["bar"] = serde_json::json!("nonsense");
    let (bar, warning) = lane_bar_with_warning(&p, Lane::Release);
    assert_eq!(bar, Bar::ZeroAboveNit);
    assert!(warning.is_some(), "an unknown bar is reported, not trusted");
}
