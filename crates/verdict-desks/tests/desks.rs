//! End-to-end tests of the desk engine with a real child provider process.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use verdict_core::{
    DecisionKind, Lane, Ledger, Policy, Record, SessionParams, Tier, Writer, start_session,
};
use verdict_desks::{CommandProvider, DeskOptions, Outcome, providers_from_policy, run};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new(label: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-desks-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn provider(name: &str, vendor: &str, args: &[&str]) -> Box<dyn verdict_desks::DeskProvider> {
    Box::new(CommandProvider {
        name: name.into(),
        vendor: vendor.into(),
        model: "test-model-1".into(),
        command: env!("CARGO_BIN_EXE_vdt-fake-desk").into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timeout_secs: Some(20),
    })
}

/// A provider whose own timeout is unset, so `DeskOptions.timeout` is the deadline the test means.
fn provider_with_desk_timeout(args: &[&str]) -> Box<dyn verdict_desks::DeskProvider> {
    Box::new(CommandProvider {
        name: "slow-with-helper".into(),
        vendor: "openai".into(),
        model: "test-model-1".into(),
        command: env!("CARGO_BIN_EXE_vdt-fake-desk").into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        timeout_secs: None,
    })
}

/// Whether a process id is still running. Used to prove a timed-out provider's child is gone.
fn process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        match std::process::Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(unix)]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}

/// A project with one session (builder vendor `anthropic`), one prompt, one claim and one check.
fn project(label: &str, policy_json: Option<&str>) -> (Tmp, Tmp, String) {
    let dir = Tmp::new(label);
    let home = Tmp::new("home");
    if let Some(p) = policy_json {
        std::fs::write(dir.path().join("verdict.json"), p).unwrap();
    }
    let mut l = Ledger::open(dir.path()).unwrap();
    let s = start_session(
        &mut l,
        dir.path(),
        &SessionParams {
            agent: "claude".into(),
            model: "builder-model".into(),
            vendor: "anthropic".into(),
            home: Some(home.path().to_path_buf()),
        },
    )
    .unwrap();
    l.append(
        Writer::Adapter,
        Some(&s.id),
        Record::Prompt {
            ordinal: 1,
            text: "make src/login.ts show an error".into(),
            agent: "claude".into(),
            model: "builder-model".into(),
        },
    )
    .unwrap();
    l.append(
        Writer::Agent,
        Some(&s.id),
        Record::Claim {
            source: verdict_core::ClaimSource::PromptTranslation,
            text: "src/login.ts shows an error on a wrong password".into(),
            names: vec!["src/login.ts".into()],
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/login.ts"), "let a = 1;\nlet b = 2;\n").unwrap();
    (dir, home, s.id)
}

#[test]
fn a_cited_answer_becomes_a_finding_and_an_uncited_one_a_hypothesis() {
    let (dir, home, sid) = project("cited", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider(
        "fake-codex",
        "openai",
        &["--record", "2", "--model", "m-2"],
    )];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran {
        receipts,
        single_vendor,
    } = out
    else {
        panic!("the desks should have run: {out:?}");
    };
    assert!(!single_vendor);
    // The standard lane's two desks: engineer and qa.
    assert_eq!(receipts.len(), 2, "{receipts:?}");
    for r in &receipts {
        assert_eq!(r.outcome, "ran");
        assert_eq!(r.vendor, "openai");
        assert_eq!(r.requested_model, "test-model-1");
        assert_eq!(r.returned_model.as_deref(), Some("m-2"));
        assert_eq!(r.findings, 1);
        assert_eq!(r.hypotheses, 1);
        assert_eq!(r.usage.as_ref().unwrap()["input_tokens"], 11);
        assert!(r.check_seq > 0);
    }
    // Findings are written by the desk, at the judgment tier, with a resolving citation.
    let records = l.records().unwrap();
    let findings: Vec<_> = records
        .iter()
        .filter_map(|s| match &s.record {
            Record::Finding(f) if f.judgment => Some((s.writer, f)),
            _ => None,
        })
        .collect();
    assert_eq!(findings.len(), 2, "{findings:?}");
    for (w, f) in &findings {
        assert_eq!(*w, Writer::Desk);
        assert_eq!(f.tier, Tier::Judgment);
        assert!(f.id.starts_with("desk:"), "{}", f.id);
        assert!(!f.evidence.is_empty());
        assert!(f.evidence[0].describe().contains("ledger record 2"));
    }
    // Hypotheses are separate records and move nothing.
    let hyps = records
        .iter()
        .filter(|s| matches!(s.record, Record::Hypothesis(_)))
        .count();
    assert_eq!(hyps, 2, "two desks, one hypothesis each");
    // Each desk leaves a receipt check record naming the provider and the served model.
    let receipts_in_ledger: Vec<_> = records
        .iter()
        .filter_map(|s| match &s.record {
            Record::Check {
                name,
                tier,
                detail: Some(d),
                ..
            } if name.starts_with("desk:") => Some((name.clone(), *tier, d.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(receipts_in_ledger.len(), 2);
    for (name, tier, d) in &receipts_in_ledger {
        assert_eq!(*tier, Tier::Judgment);
        assert!(d.contains("provider \"fake-codex\""), "{d}");
        assert!(d.contains("returned model Some(\"m-2\")"), "{d}");
        assert!(name.starts_with("desk:"));
    }
}

#[test]
fn above_the_cross_vendor_lane_a_same_vendor_provider_is_refused_and_nothing_is_written() {
    let (dir, home, sid) = project("refused", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let before = l.count().unwrap();
    let providers = vec![provider("same-vendor", "anthropic", &["--record", "2"])];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Refused { reason, desks } = out else {
        panic!("a same-vendor provider above the threshold must be refused: {out:?}");
    };
    assert!(reason.contains("cross-vendor"), "{reason}");
    assert_eq!(desks, vec!["engineer".to_string(), "qa".to_string()]);
    let records = l.records().unwrap();
    assert_eq!(records.len() as u64, before + 2, "one unavailable per desk");
    assert!(records[before as usize..].iter().all(|s| matches!(
        &s.record,
        Record::CheckUnavailable { component, .. } if component.starts_with("desk:")
    )));
    assert!(
        !records
            .iter()
            .any(|s| matches!(s.record, Record::Finding(_)))
    );
}

#[test]
fn the_global_warn_only_policy_runs_the_desks_and_records_single_vendor() {
    // The downgrade lives in the global policy, never in the project's file.
    let (dir, home, sid) = project("warn", None);
    std::fs::create_dir_all(home.path().join(".verdict")).unwrap();
    std::fs::write(
        home.path().join(".verdict/policy.json"),
        r#"{"version":1,"vendors":{"single_vendor":"warn"}}"#,
    )
    .unwrap();
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider("same-vendor", "anthropic", &["--record", "2"])];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran {
        receipts,
        single_vendor,
    } = out
    else {
        panic!("warn-only runs: {out:?}");
    };
    assert!(single_vendor, "the downgrade is recorded");
    assert_eq!(receipts.len(), 2);
    assert!(receipts.iter().all(|r| r.outcome == "ran"));
}

#[test]
fn a_project_may_not_set_the_global_single_vendor_downgrade() {
    let (dir, home, sid) = project(
        "projectwarn",
        Some(r#"{"version":1,"vendors":{"single_vendor":"warn"}}"#),
    );
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider("same-vendor", "anthropic", &["--record", "2"])];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    assert!(matches!(out, Outcome::Refused { .. }), "{out:?}");

    // Setting the default (`refuse`) explicitly in a project is not a downgrade and is allowed.
    std::fs::write(
        dir.path().join("verdict.json"),
        r#"{"version":1,"vendors":{"single_vendor":"refuse"}}"#,
    )
    .unwrap();
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    // Same vendor and `refuse`: the run is refused by the cross-vendor rule, not by the setting.
    assert!(matches!(out, Outcome::Refused { .. }), "{out:?}");
}

#[test]
fn a_failing_provider_is_a_failed_receipt_not_a_finding() {
    let (dir, home, sid) = project("fail", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider("broken", "openai", &["--fail", "2"])];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::High,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran { receipts, .. } = out else {
        panic!("{out:?}");
    };
    assert!(!receipts.is_empty());
    assert!(receipts.iter().all(|r| r.outcome == "failed"));
    let records = l.records().unwrap();
    assert!(
        !records
            .iter()
            .any(|s| matches!(s.record, Record::Finding(_)))
    );
    assert!(records.iter().any(|s| matches!(
        &s.record,
        Record::Check { name, outcome: verdict_core::CheckOutcome::FailedToRun, .. } if name.starts_with("desk:")
    )));
}

#[test]
fn a_citation_that_does_not_resolve_makes_the_answer_a_hypothesis() {
    let (dir, home, sid) = project("badfile", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider(
        "fake",
        "openai",
        &["--record", "9999", "--file", "src/missing.ts:1"],
    )];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    assert!(matches!(out, Outcome::Ran { .. }));
    let records = l.records().unwrap();
    assert!(
        !records
            .iter()
            .any(|s| matches!(s.record, Record::Finding(_))),
        "a citation that does not resolve must not become a finding"
    );
    assert!(records.iter().any(|s| matches!(
        &s.record,
        Record::Hypothesis(f) if f.title.contains("it cites")
    )));
}

#[test]
fn the_low_lane_runs_no_desks() {
    let (dir, home, sid) = project("low", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider("fake", "openai", &[])];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Low,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    assert_eq!(
        out,
        Outcome::Ran {
            receipts: Vec::new(),
            single_vendor: false
        }
    );
}

#[test]
fn a_policy_with_no_providers_runs_nothing_and_a_configured_one_is_read() {
    let (dir, home, _sid) = project("providers", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    assert!(providers_from_policy(&policy).unwrap().is_empty());
    std::fs::write(
        dir.path().join("verdict.json"),
        r#"{"version":1,"vendors":{"providers":[
            {"name":"codex","vendor":"openai","command":"codex","args":["exec"],"model":"gpt-6.1-sol","timeout_secs":120}
        ]}}"#,
    )
    .unwrap();
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let p = providers_from_policy(&policy).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].vendor, "openai");
    assert_eq!(p[0].args, vec!["exec".to_string()]);
    assert_eq!(p[0].timeout_secs, Some(120));
}

#[test]
fn a_decision_is_not_something_a_desk_writes() {
    // The ledger, not the engine, enforces this; the engine never appends one.
    let (dir, _home, sid) = project("nowrite", None);
    let mut l = Ledger::open(dir.path()).unwrap();
    let r = l.append(
        Writer::Desk,
        Some(&sid),
        Record::Decision {
            decision: DecisionKind::AcceptFindingNotReal,
            choice: "x".into(),
            reason: None,
            who: Writer::Human,
        },
    );
    assert!(r.is_err());
}

#[test]
fn an_early_helper_is_ended_by_the_tree_kill() {
    // A provider that starts its helper before reading the request can predate Windows job
    // membership. The timeout must still end that helper, not only the direct child.
    let (dir, home, sid) = project("early-tree", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let pidfile = dir.path().join("early.pid");
    let pid_path = pidfile.to_string_lossy().to_string();
    let providers = vec![provider_with_desk_timeout(&[
        "--sleep-ms",
        "60000",
        "--spawn-early-child",
        &pid_path,
    ])];
    let started = std::time::Instant::now();
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(1),
        },
        &providers,
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a timed-out provider must not hold the run past its deadline"
    );
    let Outcome::Ran { receipts, .. } = out else {
        panic!("{out:?}");
    };
    assert!(receipts.iter().all(|r| r.outcome == "failed"));
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .expect("the provider wrote its early helper's pid")
        .trim()
        .parse()
        .expect("a pid");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !process_alive(pid),
        "the provider's early helper outlived the deadline (pid {pid})"
    );
}

#[test]
fn a_timed_out_provider_ends_its_whole_process_tree() {
    // A provider that starts a helper which holds the inherited pipes. Killing only the direct
    // child would leave the helper running and the run waiting on its pipes; the whole tree has
    // to end, and the desk must read as failed rather than clean.
    let (dir, home, sid) = project("tree", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let pidfile = dir.path().join("helper.pid");
    let pid_path = pidfile.to_string_lossy().to_string();
    let providers = vec![provider_with_desk_timeout(&[
        "--sleep-ms",
        "60000",
        "--spawn-child",
        &pid_path,
    ])];
    let started = std::time::Instant::now();
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(1),
        },
        &providers,
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "a timed-out provider must not hold the run past its deadline"
    );
    let Outcome::Ran { receipts, .. } = out else {
        panic!("{out:?}");
    };
    assert!(
        receipts.iter().all(|r| r.outcome == "failed"),
        "a timed-out desk is a failed receipt: {receipts:?}"
    );
    assert!(
        !l.records()
            .unwrap()
            .iter()
            .any(|s| matches!(s.record, Record::Finding(_))),
        "nothing a timed-out provider said may count"
    );
    // The helper process itself is gone.
    let pid: u32 = std::fs::read_to_string(&pidfile)
        .expect("the provider wrote its helper's pid")
        .trim()
        .parse()
        .expect("a pid");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !process_alive(pid),
        "the provider's child outlived the deadline (pid {pid})"
    );
}

#[test]
fn a_critical_desk_finding_is_stored_as_critical_and_blocks_the_release_lane() {
    // The desk protocol's severity is five levels, and `critical` is one of them. The consumer must
    // not flatten it: in the release lane any open finding above minor fails the verdict.
    let (dir, home, sid) = project("critical-severity", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider(
        "fake-critical",
        "openai",
        &["--record", "2", "--severity", "critical"],
    )];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran { receipts, .. } = out else {
        panic!("the desks should have run: {out:?}");
    };
    assert_eq!(receipts[0].findings, 1);
    let stored: Vec<_> = l
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match &s.record {
            Record::Finding(f) if f.judgment => Some(verdict_core::verdict::FindingView {
                seq: s.seq,
                finding: f.clone(),
                status: verdict_core::verdict::FindingStatus::Open,
            }),
            _ => None,
        })
        .collect();
    assert_eq!(stored.len(), 2, "one finding per standard-lane desk");
    for v in &stored {
        assert_eq!(
            v.finding.severity,
            verdict_core::model::Severity::Critical,
            "a Critical desk finding must not be downgraded"
        );
        assert!(
            verdict_core::verdict::is_blocking_with(
                v,
                Lane::Release,
                verdict_core::lane::Bar::ZeroAboveNit
            ),
            "a Critical desk finding blocks the release lane"
        );
    }
}

#[test]
fn the_policy_verifier_order_decides_which_provider_serves_the_first_desk() {
    let policy_json = r#"{"version":1,"vendors":{"verifier_order":["openai"]}}"#;
    let (dir, home, sid) = project("order", Some(policy_json));
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    // Alphabetically "aaa-google" comes first; the policy prefers the openai vendor.
    let providers = vec![
        provider("aaa-google", "google", &["--record", "2"]),
        provider("zeta-openai", "openai", &["--record", "2"]),
    ];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran { receipts, .. } = out else {
        panic!("the desks should have run: {out:?}");
    };
    assert_eq!(receipts[0].desk, "engineer");
    assert_eq!(
        receipts[0].vendor, "openai",
        "verifier_order must be followed, not alphabetical order: {receipts:?}"
    );
}

#[test]
fn a_served_vendor_that_is_the_builder_is_refused_not_counted() {
    let (dir, home, sid) = project("identity", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider(
        "fake-openai",
        "openai",
        &["--record", "2", "--served-vendor", "anthropic"],
    )];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran { receipts, .. } = out else {
        panic!("the desks should have run: {out:?}");
    };
    assert_eq!(receipts[0].outcome, "failed");
    assert_eq!(receipts[0].identity, "mismatch");
    assert_eq!(receipts[0].served_vendor.as_deref(), Some("anthropic"));
    // The answer's findings were not counted: the run wrote no finding for that desk.
    assert_eq!(receipts[0].findings, 0);
    let findings = l
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| matches!(s.record, Record::Finding(_)))
        .count();
    assert_eq!(findings, 0);
}

#[test]
fn a_provider_with_a_large_answer_cannot_deadlock_the_parent() {
    let (dir, home, sid) = project("chatty", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    // 300 KB of leading whitespace: far more than a pipe buffer. The answer is still valid JSON.
    let providers = vec![provider(
        "chatty",
        "openai",
        &["--record", "2", "--pad", "300000"],
    )];
    let started = std::time::Instant::now();
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(30),
        },
        &providers,
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    let Outcome::Ran { receipts, .. } = out else {
        panic!("the desks should have run: {out:?}");
    };
    assert_eq!(receipts[0].outcome, "ran");
    assert!(receipts[0].findings >= 1);
}

#[test]
fn served_settings_and_usage_are_kept_in_the_receipt_and_the_check_detail() {
    let (dir, home, sid) = project("settings", None);
    let policy = Policy::load(dir.path(), Some(home.path())).unwrap();
    let mut l = Ledger::open(dir.path()).unwrap();
    let providers = vec![provider(
        "p",
        "openai",
        &[
            "--record",
            "2",
            "--served-vendor",
            "openai",
            "--model",
            "served-1",
        ],
    )];
    let out = run(
        &mut l,
        dir.path(),
        &sid,
        &policy,
        &DeskOptions {
            lane: Lane::Standard,
            timeout: Duration::from_secs(20),
        },
        &providers,
    )
    .unwrap();
    let Outcome::Ran { receipts, .. } = out else {
        panic!("the desks should have run: {out:?}");
    };
    let r = &receipts[0];
    assert_eq!(r.identity, "confirmed");
    assert_eq!(r.served_vendor.as_deref(), Some("openai"));
    assert_eq!(r.returned_model.as_deref(), Some("served-1"));
    assert!(r.settings.is_some(), "effective settings are recorded");
    assert!(r.usage.is_some(), "usage is recorded");
    let detail = l
        .records()
        .unwrap()
        .into_iter()
        .find_map(|s| match s.record {
            Record::Check { name, detail, .. } if name == "desk:engineer" => detail,
            _ => None,
        })
        .unwrap_or_default();
    assert!(detail.contains("served vendor"), "{detail}");
    assert!(detail.contains("settings"), "{detail}");
    assert!(detail.contains("usage"), "{detail}");
}
