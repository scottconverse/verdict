mod p2b_common;

use p2b_common::*;
use std::path::Path;
use verdict_core::gate::{GateOptions, GateOutcome, NoRunner, run_pre_commit, run_verdict};
use verdict_core::human::{Action, apply};
use verdict_core::*;

struct Repo {
    repo: Tmp,
    home: Tmp,
    sid: String,
}

fn repo() -> Repo {
    let repo = git_repo("gate");
    let home = Tmp::new("gatehome");
    let (_l, sid) = repo_ledger(repo.path(), home.path());
    Repo { repo, home, sid }
}

impl Repo {
    fn ledger(&self) -> Ledger {
        Ledger::open(self.repo.path()).unwrap()
    }
    fn stage(&self, rel: &str, body: &str) {
        let p = self.repo.path().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        git(self.repo.path(), &["add", rel]);
    }
    fn gate(&self, runner: &dyn verdict_core::gate::CheckRunner) -> GateOutcome {
        self.gate_at(runner, None)
    }
    fn gate_at(
        &self,
        runner: &dyn verdict_core::gate::CheckRunner,
        now_ms: Option<i64>,
    ) -> GateOutcome {
        run_pre_commit(
            &GateOptions {
                root: self.repo.path(),
                home: Some(self.home.path()),
                now_ms,
            },
            runner,
        )
        .unwrap()
    }
    fn decide(&self, action: Action) -> Result<u64, String> {
        let mut l = self.ledger();
        apply(
            self.repo.path(),
            &mut l,
            &self.sid,
            &action,
            Some(self.home.path()),
        )
    }
    fn enforce(&self) {
        let mut l = self.ledger();
        human_in(&mut l, &self.sid, DecisionKind::ModeSwitch, "enforce", None);
    }
    fn failed(&self, id: &str) {
        let mut l = self.ledger();
        finding_in(&mut l, &self.sid, id, Severity::Major);
    }
    fn gate_checks(&self) -> Vec<String> {
        self.ledger()
            .records()
            .unwrap()
            .into_iter()
            .filter_map(|s| match s.record {
                Record::Check { name, detail, .. } if name == "gate" => detail,
                _ => None,
            })
            .collect()
    }
}

#[test]
fn shadow_mode_is_the_default_and_never_holds_even_a_failed_commit() {
    let r = repo();
    r.stage("src/a.ts", "export const a = 1;\n");
    r.failed("wiring:src/a.ts:1");
    let out = r.gate(&NoRunner);
    assert_eq!(out.mode, "shadow");
    assert_eq!(
        out.exit_code, 0,
        "shadow mode always lets the commit through"
    );
    assert!(out.would_hold, "but it knows it would have held");
    assert_eq!(out.computed.as_ref().unwrap().word, VerdictWord::Failed);
    assert!(out.text.contains("VERDICT: FAILED"), "{}", out.text);
    assert!(out.text.contains("GATE: shadow mode"), "{}", out.text);
    assert!(out.text.contains("would be held"));
    let notes = r.gate_checks();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("would have held"), "{notes:?}");
    // The verdict was written by the core.
    let l = r.ledger();
    let v = l
        .records()
        .unwrap()
        .into_iter()
        .rev()
        .find(|s| matches!(s.record, Record::Verdict { .. }))
        .unwrap();
    assert_eq!(v.writer, Writer::Core);
    assert!(l.verify().unwrap().is_ok());
}

#[test]
fn enforce_mode_from_a_human_decision_holds_a_failed_commit_and_passes_a_clean_one() {
    let r = repo();
    r.stage("src/a.ts", "export const a = 1;\n");
    r.enforce();
    // Nothing wrong yet (no runner, so nothing ran): not FAILED, so not held.
    let out = r.gate(&NoRunner);
    assert_eq!(out.mode, "enforce");
    assert_eq!(out.exit_code, 0);
    assert!(!out.would_hold);
    assert_ne!(out.computed.unwrap().word, VerdictWord::Failed);

    r.failed("wiring:src/a.ts:1");
    let out = r.gate(&NoRunner);
    assert_eq!(out.exit_code, 1, "{}", out.text);
    assert!(out.text.contains("HELD"));
    assert!(out.text.contains("commit-anyway") || out.text.contains("commit anyway"));
    assert!(out.text.contains("--tree"), "{}", out.text);
    assert!(r.gate_checks().last().unwrap().contains("held this commit"));
}

#[test]
fn enforce_can_come_from_the_policy_and_the_humans_latest_choice_wins() {
    let r = repo();
    std::fs::write(
        r.repo.path().join("verdict.json"),
        r#"{"version":1,"gate":{"mode":"enforce"}}"#,
    )
    .unwrap();
    r.stage("src/a.ts", "x\n");
    r.failed("wiring:a");
    let out = r.gate(&NoRunner);
    assert_eq!(out.mode, "enforce");
    assert_eq!(out.exit_code, 1);
    // The human switches to shadow: that is their latest word.
    let mut l = r.ledger();
    human_in(&mut l, &r.sid, DecisionKind::ModeSwitch, "shadow", None);
    let out = r.gate(&NoRunner);
    assert_eq!(out.mode, "shadow");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn commit_anyway_lets_exactly_this_staged_tree_through_and_the_verdict_stays_failed() {
    let r = repo();
    r.stage("src/a.ts", "export const a = 1;\n");
    r.enforce();
    r.failed("wiring:src/a.ts:1");
    assert_eq!(r.gate(&NoRunner).exit_code, 1);

    // An empty reason is refused and writes nothing.
    let before = r.ledger().count().unwrap();
    let err = r
        .decide(Action::CommitAnyway {
            reason: "  ".into(),
            tree: None,
        })
        .unwrap_err();
    assert!(err.contains("reason"), "{err}");
    assert_eq!(r.ledger().count().unwrap(), before);

    let seq = r
        .decide(Action::CommitAnyway {
            reason: "hotfix for the outage, will fix after".into(),
            tree: None,
        })
        .unwrap();
    let out = r.gate(&NoRunner);
    assert_eq!(out.exit_code, 0, "{}", out.text);
    assert_eq!(out.commit_anyway_seq, Some(seq));
    assert_eq!(
        out.computed.as_ref().unwrap().word,
        VerdictWord::Failed,
        "the verdict stays FAILED"
    );
    assert!(out.text.contains("verdict stays FAILED"), "{}", out.text);
    assert!(
        r.gate_checks()
            .last()
            .unwrap()
            .contains("verdict stays FAILED")
    );
    // It was recorded by the human, with the reason, for the tree.
    let l = r.ledger();
    let rec = l.get(seq).unwrap().unwrap();
    assert_eq!(rec.writer, Writer::Human);
    match rec.record {
        Record::Decision {
            decision,
            reason,
            who,
            choice,
        } => {
            assert_eq!(decision, DecisionKind::CommitAnyway);
            assert_eq!(who, Writer::Human);
            assert_eq!(
                reason.as_deref(),
                Some("hotfix for the outage, will fix after")
            );
            assert!(choice.contains(&staged_tree_of(&r)), "{choice}");
        }
        other => panic!("{other:?}"),
    }
    drop(l);

    // A different staged tree is a different commit: held again.
    r.stage("src/b.ts", "export const b = 2;\n");
    let out = r.gate(&NoRunner);
    assert_eq!(
        out.exit_code, 1,
        "commit anyway covers only the exact tree: {}",
        out.text
    );
    assert_eq!(out.commit_anyway_seq, None);
}

fn staged_tree_of(r: &Repo) -> String {
    verdict_core::lane::staged_tree(r.repo.path()).unwrap()
}

#[test]
fn commit_anyway_can_name_the_tree_the_gate_printed() {
    let r = repo();
    r.stage("src/a.ts", "1\n");
    r.enforce();
    r.failed("wiring:a");
    let held = r.gate(&NoRunner);
    let tree = held.tree.clone().unwrap();
    // The human restages something else, then names the tree the gate printed.
    r.stage("src/z.ts", "2\n");
    r.decide(Action::CommitAnyway {
        reason: "named tree".into(),
        tree: Some(tree),
    })
    .unwrap();
    assert_eq!(
        r.gate(&NoRunner).exit_code,
        1,
        "the current tree is not the named one"
    );
}

#[test]
fn commit_anyway_with_nothing_failed_is_refused() {
    let r = repo();
    let err = r
        .decide(Action::CommitAnyway {
            reason: "just in case".into(),
            tree: None,
        })
        .unwrap_err();
    assert!(err.contains("not FAILED"), "{err}");
}

#[test]
fn a_pause_skips_the_gate_until_it_ends_and_the_skip_is_recorded() {
    let r = repo();
    r.stage("src/a.ts", "1\n");
    r.enforce();
    r.failed("wiring:a");
    assert_eq!(r.gate(&NoRunner).exit_code, 1);

    let seq = r.decide(Action::Pause("30m".into())).unwrap();
    let out = r.gate(&NoRunner);
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.paused.as_ref().map(|p| p.seq), Some(seq));
    assert!(out.computed.is_none(), "a paused gate computes nothing");
    assert!(out.text.contains("paused"), "{}", out.text);
    let notes = r.gate_checks();
    assert!(
        notes.last().unwrap().contains("paused by the human until"),
        "{notes:?}"
    );

    // Long after it ends, the gate runs again.
    let later = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 2 * 3600 * 1000;
    let out = r.gate_at(&NoRunner, Some(later));
    assert!(out.paused.is_none());
    assert_eq!(out.exit_code, 1);
}

#[test]
fn bad_pauses_and_other_bad_requests_are_refused_and_write_nothing() {
    let r = repo();
    let before = r.ledger().count().unwrap();
    for a in [
        Action::Pause("soon".into()),
        Action::Pause("0m".into()),
        Action::SetMode("lenient".into()),
        Action::AcceptFinding("no-such-finding".into()),
        Action::RaiseLane(Lane::Low),
    ] {
        assert!(r.decide(a.clone()).is_err(), "{a:?}");
    }
    assert_eq!(r.ledger().count().unwrap(), before);
}

#[test]
fn every_human_action_writes_a_human_decision() {
    let r = repo();
    r.failed("wiring:a");
    let before = r.ledger().count().unwrap();
    let approved_choice = verdict_core::exec_approval::approval_choice(
        &Policy::load(r.repo.path(), Some(r.home.path())).unwrap(),
    );
    let kinds = [
        (
            Action::AcceptFinding("wiring:a".into()),
            DecisionKind::AcceptFindingNotReal,
            "wiring:a",
        ),
        (
            Action::ApproveCommands,
            DecisionKind::RunProjectCommands,
            approved_choice.as_str(),
        ),
        (
            Action::RaiseLane(Lane::High),
            DecisionKind::RaiseLane,
            "high",
        ),
        (
            Action::SetMode("enforce".into()),
            DecisionKind::ModeSwitch,
            "enforce",
        ),
        (Action::Pause("2h".into()), DecisionKind::Pause, "7200s"),
    ];
    for (a, kind, choice) in kinds {
        let seq = r.decide(a).unwrap();
        let rec = r.ledger().get(seq).unwrap().unwrap();
        assert_eq!(rec.writer, Writer::Human);
        match rec.record {
            Record::Decision {
                decision,
                choice: c,
                who,
                ..
            } => {
                assert_eq!(decision, kind);
                assert_eq!(c, choice);
                assert_eq!(who, Writer::Human);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(rec.session_id.as_deref(), Some(r.sid.as_str()));
    }
    assert_eq!(r.ledger().count().unwrap(), before + 5);
}

#[test]
fn without_a_wired_runner_every_required_checker_is_unavailable_and_the_verdict_is_partial() {
    let r = repo();
    r.stage("src/a.ts", "1\n");
    let out = r.gate(&NoRunner);
    let c = out.computed.unwrap();
    assert_eq!(c.word, VerdictWord::Partial);
    assert_eq!(out.exit_code, 0);
    let unavailable: Vec<String> = r
        .ledger()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::CheckUnavailable { component, .. } => Some(component),
            _ => None,
        })
        .collect();
    for want in ["wiring", "exec", "baseline_diff"] {
        assert!(unavailable.contains(&want.to_string()), "{unavailable:?}");
    }
}

#[test]
fn a_runner_that_crashes_does_not_crash_the_gate() {
    struct Boom;
    impl verdict_core::gate::CheckRunner for Boom {
        fn run_lane(
            &self,
            _: &mut Ledger,
            _: &Path,
            _: &str,
            _: &[String],
            _: &verdict_core::gate::RunOptions,
        ) -> Result<Vec<verdict_core::gate::CheckSummary>, String> {
            panic!("checker blew up");
        }
    }
    let r = repo();
    r.stage("src/a.ts", "1\n");
    let out = r.gate(&Boom);
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.computed.unwrap().word, VerdictWord::Partial);
}

#[test]
fn the_gate_picks_the_lane_from_the_staged_files_and_hands_its_checkers_to_the_runner() {
    // Docs only: the low lane's four checkers.
    let r = repo();
    r.stage("docs/guide.md", "hi\n");
    let runner = FakeRunner::new();
    runner.set_all_static(&["wiring", "regressions", "claims_vs_code", "unfinished"]);
    let out = r.gate(&runner);
    let seen = runner.seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, Lane::Low);
    assert_eq!(
        seen[0].0,
        vec!["wiring", "regressions", "claims_vs_code", "unfinished"]
    );
    assert_eq!(out.computed.unwrap().label(), "VERIFIED (static)");

    // A route file: the high lane, `*` expanded and `-firstrun`, `-release` removed.
    let r = repo();
    r.stage("src/routes/home.ts", "x\n");
    let runner = FakeRunner::new();
    let out = r.gate(&runner);
    let seen = runner.seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, Lane::High);
    assert!(seen[0].0.contains(&"mutation".to_string()));
    assert!(!seen[0].0.contains(&"firstrun".to_string()));
    assert_eq!(out.computed.unwrap().lane, Lane::High);

    // The human raises the lane to release: the full list.
    let r = repo();
    r.stage("src/a.ts", "x\n");
    r.decide(Action::RaiseLane(Lane::Release)).unwrap();
    let runner = FakeRunner::new();
    r.gate(&runner);
    let seen = runner.seen.lock().unwrap().clone();
    assert_eq!(seen[0].1, Lane::Release);
    assert_eq!(seen[0].0.len(), verdict_core::lane::ALL_CHECKERS.len());
}

#[test]
fn a_finding_the_checker_stops_reporting_when_it_re_runs_is_resolved() {
    let r = repo();
    r.stage("docs/guide.md", "hi\n");
    let runner = FakeRunner::new();
    runner.set(
        "wiring",
        Tier::Static,
        &[("wiring:src/a.ts:1", Severity::Major)],
    );
    runner.set_all_static(&["regressions", "claims_vs_code", "unfinished"]);
    let out = r.gate(&runner);
    assert_eq!(out.computed.unwrap().word, VerdictWord::Failed);

    // The agent fixed it; wiring now reports nothing.
    runner.set("wiring", Tier::Static, &[]);
    let out = r.gate(&runner);
    let c = out.computed.unwrap();
    assert_eq!(c.word, VerdictWord::Verified, "{:#?}", c);
    assert!(out.text.contains("VERDICT: VERIFIED (static)"));
    // The old finding is still in the ledger, marked resolved by the core.
    let l = r.ledger();
    let corrections = l
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| matches!(s.record, Record::Correction { .. }))
        .count();
    assert_eq!(corrections, 1);
    let text = render_report_with(&l, Some(&r.sid), Some(&c)).unwrap();
    assert!(text.contains("resolved (correction record"), "{text}");

    // But a finding whose checker did not run again stays open.
    let r = repo();
    r.stage("docs/guide.md", "hi\n");
    r.failed("exec:npm-test");
    let runner = FakeRunner::new();
    runner.set_all_static(&["wiring", "regressions", "claims_vs_code", "unfinished"]);
    assert_eq!(r.gate(&runner).computed.unwrap().word, VerdictWord::Failed);
    // And one with no checker prefix is never auto-resolved.
    let r = repo();
    r.stage("docs/guide.md", "hi\n");
    r.failed("some-opaque-id");
    assert_eq!(r.gate(&runner).computed.unwrap().word, VerdictWord::Failed);
}

#[test]
fn with_no_session_the_gate_opens_its_own_and_fails_open() {
    let repo = git_repo("nosess");
    let home = Tmp::new("h");
    let out = run_pre_commit(
        &GateOptions {
            root: repo.path(),
            home: Some(home.path()),
            now_ms: None,
        },
        &NoRunner,
    )
    .unwrap();
    assert_eq!(out.exit_code, 0);
    let l = Ledger::open_existing(repo.path()).unwrap().unwrap();
    let sessions = list_sessions(&l).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].agent, "pre-commit-gate");
    assert!(l.verify().unwrap().is_ok());
}

#[test]
fn a_broken_policy_file_falls_back_to_the_default_and_says_so() {
    let r = repo();
    std::fs::write(r.repo.path().join("verdict.json"), "{ not json").unwrap();
    r.stage("src/a.ts", "1\n");
    let out = r.gate(&NoRunner);
    assert_eq!(out.exit_code, 0);
    let c = out.computed.unwrap();
    assert_ne!(c.word, VerdictWord::Verified);
    assert!(
        c.yours_to_settle
            .iter()
            .any(|s| s.contains("policy could not run")),
        "{:?}",
        c.yours_to_settle
    );
}

#[test]
fn vdt_verdict_computes_writes_and_names_the_session() {
    let r = repo();
    let runner = FakeRunner::new();
    runner.set_all_static(&["wiring", "regressions", "claims_vs_code", "unfinished"]);
    let (c, seq) = run_verdict(r.repo.path(), Some(r.home.path()), None, Some(&runner)).unwrap();
    assert_eq!(c.session, r.sid);
    assert_eq!(c.label(), "VERIFIED (static)");
    assert!(matches!(
        r.ledger().get(seq).unwrap().unwrap().record,
        Record::Verdict {
            word: VerdictWord::Verified,
            ..
        }
    ));
    // Without a runner it computes from what is there.
    let (c, _) = run_verdict(r.repo.path(), Some(r.home.path()), Some(&r.sid), None).unwrap();
    assert_eq!(c.word, VerdictWord::Verified);
    assert!(run_verdict(r.repo.path(), Some(r.home.path()), Some("s-nope"), None).is_err());
}
