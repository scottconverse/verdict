//! The decisions queue on the localhost page, over real TCP to the real server (loopback only).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::verdict::CommitAnywayChoice;
use verdict_core::*;
use verdict_gui::{Page, start_background_with};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-gui-dec-{}-{}",
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

struct Fixture {
    dir: Tmp,
    _home: Tmp,
    session: String,
    page: Page,
}

fn pointer() -> Vec<EvidencePointer> {
    vec![EvidencePointer::command("cargo test", b"red")]
}

/// A session whose standard lane ran every checker, with the findings given.
fn fixture_with(findings: &[(&str, &str, Severity)]) -> Fixture {
    let dir = Tmp::new();
    let home = Tmp::new();
    let mut l = Ledger::open(&dir.0).unwrap();
    let s = start_session(
        &mut l,
        &dir.0,
        &SessionParams {
            agent: "claude".into(),
            model: "model-x".into(),
            vendor: "anthropic".into(),
            home: Some(home.0.clone()),
        },
    )
    .unwrap();
    l.append(
        Writer::Adapter,
        Some(&s.id),
        Record::ToolCall {
            tool: "edit".into(),
            target: "src/login.ts".into(),
            output_hash: "0".repeat(64),
            duration_ms: 3,
        },
    )
    .unwrap();
    for (name, tier) in [
        ("wiring", Tier::Static),
        ("baseline_diff", Tier::Static),
        ("regressions", Tier::Static),
        ("tests_weakened", Tier::Static),
        ("claims_vs_code", Tier::Static),
        ("unfinished", Tier::Static),
        ("exec", Tier::Ran),
    ] {
        l.append(
            Writer::Core,
            Some(&s.id),
            Record::Check {
                name: name.into(),
                tier,
                inputs_hash: "h".into(),
                started_at: "2026-10-26T12:00:00.000Z".into(),
                finished_at: "2026-10-26T12:00:01.000Z".into(),
                outcome: CheckOutcome::Ran,
                detail: None,
            },
        )
        .unwrap();
    }
    for (id, title, severity) in findings {
        l.append(
            Writer::Checker,
            Some(&s.id),
            Record::Finding(Finding {
                id: (*id).into(),
                severity: *severity,
                title: (*title).into(),
                claim_seq: None,
                evidence: pointer(),
                tier: Tier::Static,
                blast: None,
                judgment: false,
                fix_hint: None,
            }),
        )
        .unwrap();
    }
    drop(l);
    let page = start_background_with(dir.0.clone(), 0, Some(home.0.clone())).unwrap();
    Fixture {
        dir,
        _home: home,
        session: s.id,
        page,
    }
}

fn fixture() -> Fixture {
    fixture_with(&[(
        "wiring:src/login.ts:12",
        "Login button calls /api/signin, which does not exist",
        Severity::Major,
    )])
}

struct Reply {
    status: u16,
    location: Option<String>,
    body: String,
}

fn http(
    page: &Page,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Reply {
    let mut s = TcpStream::connect(page.addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n",
        page.addr.port()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let location = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("location")
            .then(|| v.trim().to_string())
    });
    Reply {
        status,
        location,
        body: body.to_string(),
    }
}

fn get(f: &Fixture, path: &str) -> Reply {
    http(&f.page, "GET", path, &[], None)
}

fn origin(f: &Fixture) -> String {
    format!("http://127.0.0.1:{}", f.page.addr.port())
}

fn urlenc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn post(f: &Fixture, token: &str, action: &str, value: &str, reason: &str) -> Reply {
    let body = format!(
        "token={}&session={}&action={}&value={}&reason={}",
        urlenc(token),
        urlenc(&f.session),
        urlenc(action),
        urlenc(value),
        urlenc(reason)
    );
    http(
        &f.page,
        "POST",
        "/decide",
        &[("Origin", &origin(f)), ("Sec-Fetch-Site", "same-origin")],
        Some(&body),
    )
}

fn press(f: &Fixture, action: &str, value: &str, reason: &str) -> Reply {
    post(f, &f.page.token, action, value, reason)
}

fn records(dir: &Path) -> Vec<Stored> {
    Ledger::open_existing(dir)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
}

fn last_decision(dir: &Path) -> (Writer, DecisionKind, String, Option<String>, Option<String>) {
    records(dir)
        .into_iter()
        .rev()
        .find_map(|s| match s.record {
            Record::Decision {
                decision,
                choice,
                reason,
                ..
            } => Some((s.writer, decision, choice, reason, s.session_id)),
            _ => None,
        })
        .expect("a decision")
}

fn count(dir: &Path) -> usize {
    records(dir).len()
}

fn follow(f: &Fixture, r: &Reply) -> Reply {
    assert_eq!(r.status, 303, "{}", r.body);
    get(f, r.location.as_deref().expect("a Location header"))
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

// ------------------------------------------------------------------------- the page

#[test]
fn the_verdict_word_with_its_suffix_is_the_first_thing_on_the_session_page() {
    let f = fixture();
    let r = get(&f, &format!("/session/{}", f.session));
    assert_eq!(r.status, 200);
    let word_at = r.body.find("VERDICT: FAILED").expect("the verdict word");
    assert!(r.body.contains("w-failed"));
    for later in ["Waiting for you", "Your controls", "The report", "<pre>"] {
        assert!(
            word_at < r.body.find(later).unwrap(),
            "the verdict must come before {later}"
        );
    }
    // A clean session reads VERIFIED with its mandatory suffix.
    let clean = fixture_with(&[]);
    let r = get(&clean, &format!("/session/{}", clean.session));
    assert!(r.body.contains("VERDICT: VERIFIED (ran)"), "{}", r.body);
    assert!(r.body.contains("w-verified"));
    // A session where nothing ran reads UNVERIFIED, never clean.
    let dir = Tmp::new();
    let home = Tmp::new();
    let mut l = Ledger::open(&dir.0).unwrap();
    let s = start_session(
        &mut l,
        &dir.0,
        &SessionParams {
            agent: "a".into(),
            model: "m".into(),
            vendor: "v".into(),
            home: Some(home.0.clone()),
        },
    )
    .unwrap();
    drop(l);
    let page = start_background_with(dir.0.clone(), 0, Some(home.0.clone())).unwrap();
    let r = http(&page, "GET", &format!("/session/{}", s.id), &[], None);
    assert!(r.body.contains("VERDICT: UNVERIFIED"), "{}", r.body);
    assert!(!r.body.contains("VERDICT: VERIFIED"));
}

#[test]
fn the_queue_lists_what_is_waiting_and_every_phase_two_control() {
    let f = fixture();
    let b = get(&f, &format!("/session/{}", f.session)).body;
    assert!(b.contains("Login button calls /api/signin, which does not exist"));
    assert!(b.contains("Proven wrong:"));
    assert!(b.contains("Accept as not real"));
    assert!(
        b.contains("Approve this repo&#39;s commands"),
        "exec is required and not approved"
    );
    for button in [
        "Raise the lane",
        "Switch to enforce",
        "Pause the gate",
        "Commit anyway",
    ] {
        assert!(b.contains(button), "missing control: {button}");
    }
    // Hooks are still answered on the sessions page.
    assert!(b.contains("Install the claude_code hooks"));
    // Every form carries the token and posts to /decide.
    let forms = b.matches("<form").count();
    assert!(forms >= 6, "{forms}");
    assert_eq!(b.matches("action=\"/decide\"").count(), forms);
    assert_eq!(b.matches(&f.page.token).count(), forms);
}

#[test]
fn commit_anyway_is_offered_only_while_the_verdict_is_failed() {
    let clean = fixture_with(&[]);
    let b = get(&clean, &format!("/session/{}", clean.session)).body;
    assert!(b.contains("appears here when the verdict is FAILED"));
    assert!(!b.contains("name=\"reason\""));
    assert!(!b.contains("class=\"danger\""));
}

#[test]
fn text_from_the_ledger_is_escaped() {
    let f = fixture_with(&[(
        "wiring:x",
        "<script>alert(1)</script> & more",
        Severity::Major,
    )]);
    let b = get(&f, &format!("/session/{}", f.session)).body;
    assert!(!b.contains("<script>alert(1)</script>"));
    assert!(unescape(&b).contains("<script>alert(1)</script> & more"));
}

// ------------------------------------------------------------------------- the actions

#[test]
fn accepting_a_finding_as_not_real_closes_it_and_recomputes_the_verdict() {
    let f = fixture_with(&[
        ("wiring:a", "first problem", Severity::Major),
        ("regressions:b", "second problem", Severity::Critical),
    ]);
    let r = press(&f, "accept_finding", "wiring:a", "");
    let page = follow(&f, &r);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("Your decision is recorded"));
    assert!(
        page.body.contains("VERDICT: FAILED"),
        "the other finding is still open"
    );
    let d = last_decision(&f.dir.0);
    assert_eq!(
        (d.0, d.1, d.2.as_str()),
        (
            Writer::Human,
            DecisionKind::AcceptFindingNotReal,
            "wiring:a"
        )
    );
    assert_eq!(d.4.as_deref(), Some(f.session.as_str()));

    let r = press(&f, "accept_finding", "regressions:b", "");
    let page = follow(&f, &r);
    assert!(
        page.body.contains("VERDICT: VERIFIED (ran)"),
        "{}",
        page.body
    );
    // A finding that is not open cannot be accepted twice, and nothing is written.
    let before = count(&f.dir.0);
    assert_eq!(press(&f, "accept_finding", "regressions:b", "").status, 400);
    assert_eq!(press(&f, "accept_finding", "no-such", "").status, 400);
    assert_eq!(count(&f.dir.0), before);
}

#[test]
fn approving_the_repos_commands_records_it_and_the_card_goes_away() {
    let f = fixture();
    let r = press(&f, "approve_commands", "", "");
    let page = follow(&f, &r);
    assert!(!page.body.contains("Approve this repo&#39;s commands"));
    let d = last_decision(&f.dir.0);
    assert_eq!(
        (d.0, d.1),
        (Writer::Human, DecisionKind::RunProjectCommands)
    );
}

#[test]
fn raising_the_lane_records_it_and_nothing_can_lower_it() {
    let f = fixture();
    let r = press(&f, "raise_lane", "release", "");
    let page = follow(&f, &r);
    assert!(
        page.body.contains("Lane: <strong>release</strong>"),
        "{}",
        page.body
    );
    assert!(page.body.contains("you raised it"));
    // The release lane now needs checkers that never ran.
    assert!(page.body.contains("VERDICT: FAILED"));
    let d = last_decision(&f.dir.0);
    assert_eq!(
        (d.0, d.1, d.2.as_str()),
        (Writer::Human, DecisionKind::RaiseLane, "release")
    );
    // Lowering (or "raising" to the same lane) is refused and writes nothing.
    let before = count(&f.dir.0);
    for lane in ["low", "standard", "release", "bogus"] {
        assert_eq!(press(&f, "raise_lane", lane, "").status, 400, "{lane}");
    }
    assert_eq!(count(&f.dir.0), before);
}

#[test]
fn the_shadow_enforce_switch_records_a_human_mode_switch() {
    let f = fixture();
    assert!(
        get(&f, &format!("/session/{}", f.session))
            .body
            .contains("Gate mode: <strong>shadow</strong>")
    );
    let page = follow(&f, &press(&f, "set_mode", "enforce", ""));
    assert!(page.body.contains("Gate mode: <strong>enforce</strong>"));
    assert!(page.body.contains("Switch to shadow"));
    let d = last_decision(&f.dir.0);
    assert_eq!(
        (d.0, d.1, d.2.as_str()),
        (Writer::Human, DecisionKind::ModeSwitch, "enforce")
    );
    assert_eq!(current_mode_of(&f.dir.0), "enforce");
    let before = count(&f.dir.0);
    assert_eq!(press(&f, "set_mode", "lenient", "").status, 400);
    assert_eq!(count(&f.dir.0), before);
    let page = follow(&f, &press(&f, "set_mode", "shadow", ""));
    assert!(page.body.contains("Gate mode: <strong>shadow</strong>"));
}

fn current_mode_of(dir: &Path) -> String {
    verdict_core::part1c::current_mode(&Ledger::open_existing(dir).unwrap().unwrap()).unwrap()
}

#[test]
fn pausing_with_a_duration_records_it_and_the_page_says_when_it_ends() {
    let f = fixture();
    let page = follow(&f, &press(&f, "pause", "45m", ""));
    assert!(page.body.contains("paused</strong> until"), "{}", page.body);
    let d = last_decision(&f.dir.0);
    assert_eq!(
        (d.0, d.1, d.2.as_str()),
        (Writer::Human, DecisionKind::Pause, "2700s")
    );
    let before = count(&f.dir.0);
    for bad in ["", "soon", "0m", "5"] {
        assert_eq!(press(&f, "pause", bad, "").status, 400, "{bad:?}");
    }
    assert_eq!(count(&f.dir.0), before);
}

#[test]
fn commit_anyway_needs_a_typed_reason_and_leaves_the_verdict_failed() {
    let f = fixture();
    let before = count(&f.dir.0);
    for empty in ["", "   "] {
        let r = press(&f, "commit_anyway", "", empty);
        assert_eq!(r.status, 400, "{}", r.body);
        assert!(r.body.contains("reason"));
    }
    assert_eq!(count(&f.dir.0), before, "an empty reason writes nothing");

    let r = press(&f, "commit_anyway", "", "the outage is worse than this bug");
    let page = follow(&f, &r);
    assert!(
        page.body.contains("VERDICT: FAILED"),
        "the verdict stays FAILED"
    );
    assert!(page.body.contains("commit-anyway override open"));
    assert!(page.body.contains("the outage is worse than this bug"));
    let d = last_decision(&f.dir.0);
    assert_eq!((d.0, d.1), (Writer::Human, DecisionKind::CommitAnyway));
    assert_eq!(d.3.as_deref(), Some("the outage is worse than this bug"));
    let c = CommitAnywayChoice::decode(&d.2);
    assert_eq!(c.finding_ids, vec!["wiring:src/login.ts:12"]);
    assert_eq!(
        c.tree, "none",
        "this folder is not a git repository, so there is no staged tree"
    );

    // With nothing FAILED there is nothing to commit past.
    let clean = fixture_with(&[]);
    let before = count(&clean.dir.0);
    assert_eq!(
        press(&clean, "commit_anyway", "", "just because").status,
        400
    );
    assert_eq!(count(&clean.dir.0), before);
}

#[test]
fn every_action_writes_exactly_one_decision_as_the_human() {
    let f = fixture();
    let cases: [(&str, &str, &str); 5] = [
        ("approve_commands", "", ""),
        ("raise_lane", "high", ""),
        ("set_mode", "enforce", ""),
        ("pause", "10m", ""),
        ("commit_anyway", "", "typed reason"),
    ];
    for (action, value, reason) in cases {
        let before = records(&f.dir.0);
        let r = press(&f, action, value, reason);
        assert_eq!(r.status, 303, "{action}: {}", r.body);
        let after = records(&f.dir.0);
        assert_eq!(after.len(), before.len() + 1, "{action}");
        let s = after.last().unwrap();
        assert_eq!(s.writer, Writer::Human, "{action}");
        assert!(
            matches!(
                &s.record,
                Record::Decision {
                    who: Writer::Human,
                    ..
                }
            ),
            "{action}"
        );
    }
    assert!(
        Ledger::open_existing(&f.dir.0)
            .unwrap()
            .unwrap()
            .verify()
            .unwrap()
            .is_ok()
    );
}

// ------------------------------------------------------------------------- the guard

#[test]
fn no_action_works_without_the_token_or_from_another_origin() {
    let f = fixture();
    let before = count(&f.dir.0);
    let own_origin = origin(&f);
    let actions: [(&str, &str, &str); 6] = [
        ("accept_finding", "wiring:src/login.ts:12", ""),
        ("approve_commands", "", ""),
        ("raise_lane", "high", ""),
        ("set_mode", "enforce", ""),
        ("pause", "30m", ""),
        ("commit_anyway", "", "a reason"),
    ];
    for (action, value, reason) in actions {
        // Wrong or missing token, right origin.
        for token in ["", "not-the-token"] {
            let r = post(&f, token, action, value, reason);
            assert_eq!(r.status, 403, "{action} with token {token:?}: {}", r.body);
        }
        // Right token, but the request did not come from this page.
        let body = format!(
            "token={}&session={}&action={action}&value={}&reason={}",
            f.page.token,
            f.session,
            urlenc(value),
            urlenc(reason)
        );
        for headers in [
            vec![],
            vec![("Origin", "http://evil.example")],
            vec![("Origin", "http://127.0.0.1:1")],
            vec![
                ("Origin", own_origin.as_str()),
                ("Sec-Fetch-Site", "cross-site"),
            ],
        ] {
            let r = http(&f.page, "POST", "/decide", &headers, Some(&body));
            assert_eq!(r.status, 403, "{action} with {headers:?}: {}", r.body);
        }
    }
    assert_eq!(
        count(&f.dir.0),
        before,
        "nothing refused may write anything"
    );
    // A GET can never record a decision.
    let r = get(&f, "/decide");
    assert_eq!(r.status, 405);
    assert_eq!(count(&f.dir.0), before);
}

#[test]
fn an_unknown_action_or_session_is_refused_and_writes_nothing() {
    let f = fixture();
    let before = count(&f.dir.0);
    assert_eq!(press(&f, "write_verdict", "CLEAR TO SHIP", "").status, 400);
    assert_eq!(press(&f, "", "", "").status, 400);
    let body = format!(
        "token={}&session=s-nope&action=approve_commands",
        f.page.token
    );
    let r = http(
        &f.page,
        "POST",
        "/decide",
        &[("Origin", &origin(&f))],
        Some(&body),
    );
    assert_eq!(r.status, 400);
    assert_eq!(count(&f.dir.0), before);
}

#[test]
fn the_page_shows_the_commands_in_big_text_and_the_approval_names_them() {
    let f = fixture();
    let page = format!("/session/{}", f.session);
    let b = get(&f, &page).body;
    let cmd_at = b
        .find("npm run typecheck")
        .expect("the commands are listed");
    let button_at = b
        .find("Approve this repo&#39;s commands")
        .expect("the button");
    assert!(cmd_at < button_at, "the commands come before the button");
    assert!(b.contains("font-size:26px"), "the commands are in big text");

    let r = press(&f, "approve_commands", "", "");
    let after = follow(&f, &r);
    assert!(!after.body.contains("Approve this repo&#39;s commands"));
    let (_, _, choice, _, _) = last_decision(&f.dir.0);
    let policy = Policy::load(&f.dir.0, Some(&f._home.0)).unwrap();
    assert_eq!(choice, exec_approval::approval_choice(&policy));
    assert!(choice.starts_with("approved:") && choice.len() > 20);

    // The commands change: the old approval no longer counts and the page asks again.
    std::fs::write(
        f.dir.0.join("verdict.json"),
        r#"{"version":1,"exec":{"commands":[{"run":"echo sneaky-new-command","tier":"ran"}]}}"#,
    )
    .unwrap();
    let again = get(&f, &page).body;
    assert!(
        again.contains("The commands changed since you approved them"),
        "{again}"
    );
    assert!(again.contains("echo sneaky-new-command"));
    assert!(again.contains("Approve this repo&#39;s commands"));
}
