//! End-to-end tests of `vdt mcp`: a real child process, real MCP JSON-RPC over stdio.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use verdict_core::*;

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-mcp-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A project with one session and one prompt, as the hook adapter would have left it.
fn project_with_session() -> (Tmp, Tmp, String) {
    let t = Tmp::new();
    let home = Tmp::new();
    let mut l = Ledger::open(&t.0).unwrap();
    let s = start_session(
        &mut l,
        &t.0,
        &SessionParams {
            agent: "claude".into(),
            model: "m".into(),
            vendor: "anthropic".into(),
            home: Some(home.0.clone()),
        },
    )
    .unwrap();
    l.append(
        Writer::Adapter,
        Some(&s.id),
        Record::Prompt {
            ordinal: 1,
            text: "make the login page show an error in src/login.rs".into(),
            agent: "claude".into(),
            model: "m".into(),
        },
    )
    .unwrap();
    (t, home, s.id)
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Mcp {
    fn start(project: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vdt"))
            .arg("mcp")
            .arg("--project")
            .arg(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut m = Mcp {
            child,
            stdin,
            lines: rx,
            next_id: 1,
        };
        let init = m.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "vdt-test-client", "version": "0.0.0"}
            }),
        );
        assert!(init.get("result").is_some(), "initialize failed: {init}");
        m.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        m
    }

    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .expect("no answer from vdt mcp within 30 seconds");
            let v: Value = serde_json::from_str(&line).expect("server wrote non-JSON on stdout");
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    }

    /// Call a tool; return (is_error, text).
    fn call(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        let result = r
            .get("result")
            .unwrap_or_else(|| panic!("tool call {name} gave a protocol error: {r}"));
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text in {result}"))
            .to_string();
        (result["isError"].as_bool().unwrap_or(false), text)
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn vdt_stdout(dir: &Path, args: &[&str]) -> (bool, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_vdt"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).to_string(),
    )
}

#[test]
fn mcp_round_trip_against_the_real_binary() {
    let (t, _home, sid) = project_with_session();
    let mut m = Mcp::start(&t.0);

    // tools/list: exactly the five tools, nothing that could write a verdict.
    let list = m.request("tools/list", json!({}));
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "verdict_check",
            "verdict_claim",
            "verdict_decisions",
            "verdict_done_when",
            "verdict_scan"
        ]
    );

    // verdict_claim: accepted when it names a file, recorded with writer "agent".
    let (err, text) = m.call(
        "verdict_claim",
        json!({"text": "src/login.rs shows an error message when the password is wrong"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("nothing has been checked"), "{text}");
    // ... and refused, with a reason, when it names nothing.
    let (err, text) = m.call("verdict_claim", json!({"text": "the login works great"}));
    assert!(err);
    assert!(text.contains("names no URL, file or symbol"), "{text}");

    // verdict_done_when: a condition that cannot be wrong is refused.
    let (err, text) = m.call("verdict_done_when", json!({"text": "it works better"}));
    assert!(err);
    assert!(text.contains("cannot be wrong"), "{text}");
    let (err, text) = m.call(
        "verdict_done_when",
        json!({"text": "http://localhost:3000/login shows the words \"Wrong password\""}),
    );
    assert!(!err, "{text}");

    // What reached the ledger: two agent claims, nothing else from the agent.
    let l = Ledger::open_existing(&t.0).unwrap().unwrap();
    let agent_rows: Vec<_> = l
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| s.writer == Writer::Agent)
        .collect();
    assert_eq!(agent_rows.len(), 2, "{agent_rows:?}");
    assert!(
        agent_rows
            .iter()
            .all(|s| s.session_id.as_deref() == Some(sid.as_str()))
    );
    assert!(matches!(
        &agent_rows[0].record,
        Record::Claim { source: ClaimSource::PromptTranslation, names, prompt_ordinal: Some(1), .. }
            if names == &vec!["src/login.rs".to_string()]
    ));
    assert!(matches!(
        &agent_rows[1].record,
        Record::Claim {
            source: ClaimSource::DoneWhen,
            ..
        }
    ));
    drop(l);

    // verdict_scan: the same report as `vdt scan`, and it writes nothing to the ledger.
    let before = Ledger::open_existing(&t.0)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
        .len();
    let (err, text) = m.call("verdict_scan", json!({}));
    assert!(!err, "{text}");
    let after = Ledger::open_existing(&t.0)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
        .len();
    assert_eq!(before, after, "verdict_scan must write nothing: {text}");
    assert!(text.contains("SCAN: "), "{text}");
    assert!(text.contains("nothing written to the ledger"), "{text}");
    assert!(text.contains("NOT ESTABLISHED"), "{text}");
    assert!(text.contains("wiring: Ran, tier static: "), "{text}");
    for bad in ["VERDICT: VERIFIED", "CLEAR TO SHIP", "all clear"] {
        assert!(!text.contains(bad), "verdict_scan said {bad:?}: {text}");
    }
    let (ok, cli_scan) = vdt_stdout(&t.0, &["scan"]);
    assert!(ok);
    assert!(
        text.ends_with(&cli_scan),
        "verdict_scan must end with exactly what `vdt scan` prints.\n--- tool ---\n{text}\n--- cli ---\n{cli_scan}"
    );

    // verdict_check: the same report as `vdt report`, honestly not established.
    {
        let tool = "verdict_check";
        let (err, text) = m.call(tool, json!({}));
        assert!(!err, "{text}");
        assert!(text.contains("NOT ESTABLISHED"), "{tool}: {text}");
        assert!(text.contains("VERDICT: UNVERIFIED"), "{tool}: {text}");
        for bad in [
            "VERDICT: VERIFIED",
            "CLEAR TO SHIP",
            "all clear",
            "no problems",
        ] {
            assert!(!text.contains(bad), "{tool} said {bad:?}: {text}");
        }
    }
    // verdict_check wrote the verdict (as the core) once; asking again writes nothing new, so the
    // tool and the command line print exactly the same report.
    m.call("verdict_check", json!({}));
    let (ok, cli_report) = vdt_stdout(&t.0, &["report"]);
    assert!(ok);
    let (_, text) = m.call("verdict_check", json!({}));
    assert!(
        text.ends_with(&cli_report),
        "verdict_check must end with exactly what `vdt report` prints.\n--- tool ---\n{text}\n--- cli ---\n{cli_report}"
    );
    assert!(text.contains("the verdict is UNVERIFIED"), "{text}");
    assert!(!cli_report.contains("check unavailable: verdict_check"));
    assert!(!cli_report.contains("check unavailable: verdict_scan"));
    let verdicts = Ledger::open_existing(&t.0)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter(|s| matches!(s.record, Record::Verdict { .. }))
        .collect::<Vec<_>>();
    assert!(!verdicts.is_empty());
    assert!(
        verdicts.iter().all(|s| s.writer == Writer::Core),
        "only the core writes verdicts"
    );

    // verdict_decisions: read only; lists the human's open decisions.
    let (err, text) = m.call("verdict_decisions", json!({}));
    assert!(!err);
    assert!(text.contains("install_hooks"), "{text}");
    assert!(text.contains("Mode: shadow"), "{text}");
    let decisions_tool = tools
        .iter()
        .find(|t| t["name"] == "verdict_decisions")
        .unwrap();
    assert_eq!(decisions_tool["annotations"]["readOnlyHint"], json!(true));

    // The ledger is still intact after all of that.
    let (ok, verify) = vdt_stdout(&t.0, &["ledger", "verify"]);
    assert!(ok, "{verify}");
}

#[test]
fn agent_cannot_write_finding_decision_verdict_mode_or_commit_anyway_over_mcp() {
    let (t, _home, sid) = project_with_session();
    let mut m = Mcp::start(&t.0);
    let before = Ledger::open_existing(&t.0)
        .unwrap()
        .unwrap()
        .count()
        .unwrap();

    let attempts = [
        (
            "verdict_finding",
            json!({"title": "the login is fine", "id": "F-1"}),
        ),
        ("verdict_verdict", json!({})),
        ("verdict_commit_anyway", json!({"reason": "I say so"})),
        ("verdict_set_mode", json!({"choice": "enforce"})),
        ("verdict_decide", json!({"choice": "accept"})),
        ("verdict_accept_finding", json!({"choice": "F-1"})),
    ];
    for (name, args) in &attempts {
        let (err, text) = m.call(name, args.clone());
        assert!(err, "{name} must be an error: {text}");
        assert!(text.contains("REFUSED"), "{name}: {text}");
        assert!(text.contains("refused_write"), "{name}: {text}");
    }
    // None of them is advertised, and a made-up name is a protocol error.
    let r = m.request(
        "tools/call",
        json!({"name": "verdict_nonsense", "arguments": {}}),
    );
    assert!(r.get("error").is_some(), "{r}");

    let l = Ledger::open_existing(&t.0).unwrap().unwrap();
    let rows = l.records().unwrap();
    assert_eq!(
        rows.len() as u64,
        before + attempts.len() as u64,
        "each attempt adds exactly one record, a refused_write"
    );
    let new_rows = &rows[before as usize..];
    for s in new_rows {
        assert_eq!(s.writer, Writer::Core, "refusals are written by the core");
        assert_eq!(s.session_id.as_deref(), Some(sid.as_str()));
    }
    let refused: Vec<_> = new_rows
        .iter()
        .map(|s| match &s.record {
            Record::RefusedWrite {
                attempted_by,
                attempted_kind,
                attempted_decision,
                ..
            } => {
                assert_eq!(*attempted_by, Writer::Agent);
                (*attempted_kind, *attempted_decision)
            }
            other => panic!("expected refused_write, found {other:?}"),
        })
        .collect();
    assert_eq!(
        refused,
        vec![
            (RecordKind::Finding, None),
            (RecordKind::Verdict, None),
            (RecordKind::Decision, Some(DecisionKind::CommitAnyway)),
            (RecordKind::Decision, Some(DecisionKind::ModeSwitch)),
            (
                RecordKind::Decision,
                Some(DecisionKind::AcceptFindingNotReal)
            ),
            (
                RecordKind::Decision,
                Some(DecisionKind::AcceptFindingNotReal)
            ),
        ]
    );
    // No finding, decision or verdict exists anywhere in the ledger.
    assert!(!rows.iter().any(|s| matches!(
        s.record,
        Record::Finding(_)
            | Record::Hypothesis(_)
            | Record::Decision { .. }
            | Record::Verdict { .. }
    )));
    // The refusals show up in the report the human reads.
    let (_, report) = vdt_stdout(&t.0, &["report"]);
    assert!(report.contains("refused write: agent tried to write a decision (commit_anyway)"));
    assert!(report.contains("refused write: agent tried to write a decision (mode_switch)"));
    drop(l);
    let (ok, _) = vdt_stdout(&t.0, &["ledger", "verify"]);
    assert!(ok);
}

#[test]
fn with_no_session_a_claim_is_refused_and_a_check_is_not_established() {
    let t = Tmp::new();
    let mut m = Mcp::start(&t.0);
    let (err, text) = m.call("verdict_claim", json!({"text": "src/a.rs exists"}));
    assert!(err);
    assert!(text.contains("No session is open"), "{text}");
    let (err, text) = m.call("verdict_check", json!({}));
    assert!(!err);
    assert!(text.contains("NOT ESTABLISHED"), "{text}");
    assert!(text.contains("not a pass"), "{text}");
    // The server wrote nothing about a claim; the only ledger content (if any) is no record.
    if let Some(l) = Ledger::open_existing(&t.0).unwrap() {
        assert_eq!(l.count().unwrap(), 0);
    }
}
