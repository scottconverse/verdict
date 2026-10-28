//! Tests of the localhost page, over real TCP to the real server (loopback only).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use verdict_core::*;
use verdict_gui::{Page, start_background};

static N: AtomicU64 = AtomicU64::new(0);

struct Tmp(PathBuf);
impl Tmp {
    fn new() -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "vdt-gui-test-{}-{}",
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

fn fixture() -> Fixture {
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
        Record::Prompt {
            ordinal: 1,
            text: "add a <b>login</b> page & show errors".into(),
            agent: "claude".into(),
            model: "model-x".into(),
        },
    )
    .unwrap();
    l.append(
        Writer::Agent,
        Some(&s.id),
        Record::Claim {
            source: ClaimSource::PromptTranslation,
            text: "src/login.rs shows an error".into(),
            names: vec!["src/login.rs".into()],
            prompt_ordinal: Some(1),
        },
    )
    .unwrap();
    // An agent tries to write a verdict: refused, and recorded.
    let _ = l.append(
        Writer::Agent,
        Some(&s.id),
        Record::Decision {
            decision: DecisionKind::CommitAnyway,
            choice: "x".into(),
            reason: Some("because".into()),
            who: Writer::Human,
        },
    );
    drop(l);
    let page = verdict_gui::start_background_with(dir.0.clone(), 0, Some(home.0.clone())).unwrap();
    Fixture {
        dir,
        _home: home,
        session: s.id,
        page,
    }
}

struct Reply {
    status: u16,
    headers: String,
    body: String,
}

fn http(
    page: &Page,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&str>,
    host: Option<&str>,
) -> Reply {
    let mut s = TcpStream::connect(page.addr).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    let default_host = format!("127.0.0.1:{}", page.addr.port());
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        host.unwrap_or(&default_host)
    );
    for (k, v) in extra_headers {
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
    let (head, body) = text
        .split_once("\r\n\r\n")
        .map(|(h, b)| (h.to_string(), b.to_string()))
        .unwrap_or_default();
    Reply {
        status,
        headers: head,
        body,
    }
}

fn get(page: &Page, path: &str) -> Reply {
    http(page, "GET", path, &[], None, None)
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn decisions(dir: &Path) -> Vec<(Writer, DecisionKind, String)> {
    Ledger::open_existing(dir)
        .unwrap()
        .unwrap()
        .records()
        .unwrap()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::Decision {
                decision, choice, ..
            } => Some((s.writer, decision, choice)),
            _ => None,
        })
        .collect()
}

fn install_body(f: &Fixture, host: &str, token: &str) -> String {
    let _ = f;
    format!("host={host}&token={token}")
}

fn origin(f: &Fixture) -> String {
    format!("http://127.0.0.1:{}", f.page.addr.port())
}

#[test]
fn the_readyz_route_says_which_app_and_project_this_is() {
    // The installer only opens the page when this route says the server is Verdict and it serves
    // the chosen project: an unrelated process on the port must not pass.
    let f = fixture();
    let reply = get(&f.page, "/readyz");
    assert_eq!(reply.status, 200);
    assert!(
        reply
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"),
        "{}",
        reply.headers
    );
    let json: serde_json::Value = serde_json::from_str(&reply.body).expect("readiness JSON");
    assert_eq!(json["app"], "verdict");
    let served = std::fs::canonicalize(&f.dir.0).unwrap();
    assert_eq!(
        json["project"].as_str().unwrap(),
        served.to_string_lossy(),
        "the canonical project root"
    );
    assert_eq!(json["pid"].as_u64().unwrap(), u64::from(std::process::id()));
}

#[test]
fn the_packet_and_sarif_routes_serve_the_exports_as_downloads() {
    let f = fixture();
    let packet = get(&f.page, &format!("/session/{}/packet", f.session));
    assert_eq!(packet.status, 200);
    assert!(
        packet
            .headers
            .to_ascii_lowercase()
            .contains("content-type: text/plain"),
        "{}",
        packet.headers
    );
    assert!(
        packet
            .headers
            .to_ascii_lowercase()
            .contains("content-disposition: attachment"),
        "{}",
        packet.headers
    );
    assert!(
        packet.body.contains("VERDICT ACCEPTANCE PACKET"),
        "{}",
        packet.body
    );
    assert!(packet.body.contains("FINDINGS"), "{}", packet.body);

    let sarif = get(&f.page, &format!("/session/{}/sarif", f.session));
    assert_eq!(sarif.status, 200);
    assert!(
        sarif
            .headers
            .to_ascii_lowercase()
            .contains("application/sarif+json"),
        "{}",
        sarif.headers
    );
    let v: serde_json::Value = serde_json::from_str(&sarif.body).expect("the download is JSON");
    assert_eq!(v["version"], "2.1.0");
    assert!(v["runs"][0]["results"].is_array());

    // The session page links to both.
    let page = get(&f.page, &format!("/session/{}", f.session)).body;
    assert!(page.contains(&format!("/session/{}/packet", f.session)));
    assert!(page.contains(&format!("/session/{}/sarif", f.session)));
}

#[test]
fn binds_loopback_only_never_all_interfaces() {
    let f = fixture();
    assert_eq!(
        f.page.addr.ip(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    );
    assert!(!f.page.addr.ip().is_unspecified());
    // A second bind through the same function, on any port, is also loopback.
    let l = verdict_gui::bind(0).unwrap();
    assert!(l.local_addr().unwrap().ip().is_loopback());
    assert!(!l.local_addr().unwrap().ip().is_unspecified());
}

#[test]
fn route_home_lists_sessions_and_a_hooks_button_per_host() {
    let f = fixture();
    let r = get(&f.page, "/");
    assert_eq!(r.status, 200);
    assert!(
        r.body.contains(&format!("/session/{}", f.session)),
        "{}",
        r.body
    );
    assert!(r.body.contains("claude / model-x"));
    assert_eq!(r.body.matches("Install hooks in this project").count(), 4);
    for host in ["Claude Code", "Codex", "Cursor", "Antigravity"] {
        assert!(r.body.contains(host), "{host} is missing");
    }
    assert!(r.body.contains(&f.page.token));
    // The page is read only: its only forms post to the install route.
    assert_eq!(r.body.matches("<form").count(), 4);
    assert_eq!(r.body.matches("action=\"/install-hooks\"").count(), 4);
}

#[test]
fn route_home_with_no_ledger_says_so_and_creates_nothing() {
    let dir = Tmp::new();
    let page = start_background(dir.0.clone(), 0).unwrap();
    let r = get(&page, "/");
    assert_eq!(r.status, 200);
    assert!(r.body.contains("No ledger in this project yet"));
    assert!(r.body.contains("Nothing has been checked"));
    assert!(
        !dir.0.join(".verdict").exists(),
        "viewing must not create files"
    );
}

#[test]
fn route_session_shows_the_same_report_as_vdt_report() {
    let f = fixture();
    let r = get(&f.page, &format!("/session/{}", f.session));
    assert_eq!(r.status, 200);
    let expected = {
        let l = Ledger::open_existing(&f.dir.0).unwrap().unwrap();
        verdict_core::verdict::report_for_project(&f.dir.0, &l, Some(&f.session), Some(&f._home.0))
            .unwrap()
    };
    let shown = unescape(&r.body);
    for line in expected.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            shown.contains(line.trim_end()),
            "report line missing: {line}"
        );
    }
    assert!(shown.contains("VERDICT: UNVERIFIED"));
    assert!(shown.contains("NOT ESTABLISHED"));
    // Text from the ledger is escaped, never injected as HTML.
    assert!(!r.body.contains("<b>login</b>"));
    // Unknown session: 404, not an empty report.
    assert_eq!(get(&f.page, "/session/s-nope").status, 404);
}

#[test]
fn route_log_shows_every_record_and_marks_refused_writes() {
    let f = fixture();
    let r = get(&f.page, &format!("/session/{}/log", f.session));
    assert_eq!(r.status, 200);
    let shown = unescape(&r.body);
    assert!(shown.contains("session opened: claude/model-x"));
    assert!(shown.contains("prompt 1: add a <b>login</b> page & show errors"));
    assert!(!r.body.contains("<b>login</b>"), "must be escaped");
    assert!(shown.contains("claim (prompt_translation): src/login.rs shows an error"));
    assert!(shown.contains("REFUSED: agent tried to write a decision (commit_anyway)"));
    assert!(r.body.contains("class=\"refused\""));
    assert_eq!(get(&f.page, "/session/s-nope/log").status, 404);
}

#[test]
fn route_hooks_shows_the_block_for_each_host() {
    let f = fixture();
    let r = get(&f.page, "/hooks/claude_code");
    assert_eq!(r.status, 200);
    let t = unescape(&r.body);
    assert!(t.contains("vdt hook claude UserPromptSubmit"));
    assert!(t.contains("vdt hook claude Stop"));
    assert!(t.contains(".claude/settings.json"));
    let r = get(&f.page, "/hooks/codex");
    assert_eq!(r.status, 200);
    assert!(unescape(&r.body).contains("vdt hook codex PostToolUse"));
    assert_eq!(get(&f.page, "/hooks/vim").status, 404);
}

#[test]
fn pressing_install_hooks_records_a_human_decision_and_shows_the_block() {
    let f = fixture();
    let before = decisions(&f.dir.0).len();
    let r = http(
        &f.page,
        "POST",
        "/install-hooks",
        &[("Origin", &origin(&f)), ("Sec-Fetch-Site", "same-origin")],
        Some(&install_body(&f, "claude_code", &f.page.token)),
        None,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("Your decision is recorded"));
    assert!(unescape(&r.body).contains("vdt hook claude Stop"));
    let d = decisions(&f.dir.0);
    assert_eq!(d.len(), before + 1);
    assert_eq!(
        d.last().unwrap(),
        &(
            Writer::Human,
            DecisionKind::InstallHooks,
            "claude_code".to_string()
        )
    );
    // The button is wired end to end: the project's Claude Code settings now hold the three
    // entries, and the file is valid JSON.
    let settings = std::fs::read_to_string(f.dir.0.join(".claude/settings.json"))
        .expect("the install button writes the project's Claude Code settings");
    assert!(settings.contains("vdt hook claude Stop"), "{settings}");
    let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert!(parsed["hooks"]["PostToolUse"].is_array(), "{settings}");
    // Codex goes to the isolated Codex home, never the real one.
    let r = http(
        &f.page,
        "POST",
        "/install-hooks",
        &[("Origin", &origin(&f)), ("Sec-Fetch-Site", "same-origin")],
        Some(&install_body(&f, "codex", &f.page.token)),
        None,
    );
    assert_eq!(r.status, 200, "{}", r.body);
    let codex = std::fs::read_to_string(f._home.0.join(".codex/hooks.json"))
        .expect("the install button writes the Codex home hooks.json");
    assert!(codex.contains("vdt hook codex Stop"), "{codex}");
    // The home page now says so.
    assert!(
        get(&f.page, "/")
            .body
            .contains("You asked to install these hooks")
    );
    // The ledger is still intact.
    assert!(
        Ledger::open_existing(&f.dir.0)
            .unwrap()
            .unwrap()
            .verify()
            .unwrap()
            .is_ok()
    );
}

#[test]
fn a_cross_origin_post_is_refused_and_writes_nothing() {
    let f = fixture();
    let before = Ledger::open_existing(&f.dir.0)
        .unwrap()
        .unwrap()
        .count()
        .unwrap();
    let good_body = install_body(&f, "codex", &f.page.token);
    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        (
            "another site's Origin",
            vec![("Origin", "http://evil.example".to_string())],
        ),
        (
            "an Origin on another port of localhost",
            vec![("Origin", "http://127.0.0.1:1".to_string())],
        ),
        ("no Origin at all", vec![]),
        (
            "right Origin but the browser says cross-site",
            vec![
                ("Origin", origin(&f)),
                ("Sec-Fetch-Site", "cross-site".to_string()),
            ],
        ),
    ];
    for (label, headers) in cases {
        let hs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let r = http(
            &f.page,
            "POST",
            "/install-hooks",
            &hs,
            Some(&good_body),
            None,
        );
        assert_eq!(r.status, 403, "{label}: {}", r.body);
    }
    // Right Origin, wrong or missing token.
    for body in [
        install_body(&f, "codex", "not-the-token"),
        "host=codex&token=".to_string(),
    ] {
        let r = http(
            &f.page,
            "POST",
            "/install-hooks",
            &[("Origin", &origin(&f))],
            Some(&body),
            None,
        );
        assert_eq!(r.status, 403, "{}", r.body);
    }
    // A request addressed to some other host name (DNS rebinding) is refused, even for GET.
    let r = http(&f.page, "GET", "/", &[], None, Some("evil.example"));
    assert_eq!(r.status, 403);
    let after = Ledger::open_existing(&f.dir.0)
        .unwrap()
        .unwrap()
        .count()
        .unwrap();
    assert_eq!(before, after, "no refused request may write anything");
}

#[test]
fn the_page_needs_no_internet_and_uses_large_high_contrast_text() {
    let f = fixture();
    let mut bodies = vec![
        get(&f.page, "/").body,
        get(&f.page, &format!("/session/{}", f.session)).body,
        get(&f.page, &format!("/session/{}/log", f.session)).body,
        get(&f.page, "/hooks/codex").body,
    ];
    for b in &bodies {
        for needle in [
            "src=\"http",
            "href=\"http",
            "url(",
            "@import",
            "<script",
            "<link",
        ] {
            assert!(
                !b.contains(needle),
                "page reaches outside itself via {needle}"
            );
        }
    }
    let html = bodies.remove(0);
    let css = html
        .split("<style>")
        .nth(1)
        .and_then(|s| s.split("</style>").next())
        .expect("embedded css");

    // Every font size is at least 18px; the body text is 20px.
    let mut sizes = Vec::new();
    let mut rest = css;
    while let Some(i) = rest.find("font-size:") {
        let after = &rest[i + "font-size:".len()..];
        let num: String = after
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        sizes.push(num.parse::<u32>().unwrap());
        rest = after;
    }
    assert!(!sizes.is_empty());
    assert!(
        sizes.iter().all(|s| *s >= 18),
        "a font size is under 18px: {sizes:?}"
    );
    assert!(css.contains("body {") && css.contains("font-size: 20px"));

    // Every colour pair declared together meets WCAG AA (4.5:1); a lone colour is checked
    // against white, a lone background against the default text colour.
    fn rgb(h: &str) -> [f64; 3] {
        let h = h.trim_start_matches('#');
        let c = |i: usize| {
            let v = u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f64 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        [c(0), c(2), c(4)]
    }
    fn lum(h: &str) -> f64 {
        let [r, g, b] = rgb(h);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
    fn ratio(a: &str, b: &str) -> f64 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }
    fn prop(block: &str, name: &str) -> Option<String> {
        for decl in block.split(';') {
            let d = decl.trim();
            if let Some(v) = d.strip_prefix(name)
                && let Some(v) = v.strip_prefix(':')
                && let Some(h) = v.split_whitespace().find(|w| w.starts_with('#'))
            {
                return Some(h.to_string());
            }
        }
        None
    }
    let mut checked = 0;
    for rule in css.split('}') {
        let Some((sel, block)) = rule.split_once('{') else {
            continue;
        };
        if sel.trim().starts_with(":root") {
            continue;
        }
        let fg = prop(block, "color");
        let bg = prop(block, "background");
        let (fg, bg) = match (fg, bg) {
            (Some(f), Some(b)) => (f, b),
            (Some(f), None) => (f, "#ffffff".to_string()),
            (None, Some(b)) => ("#111111".to_string(), b),
            (None, None) => continue,
        };
        let r = ratio(&fg, &bg);
        assert!(r >= 4.5, "{}: {fg} on {bg} is only {r:.2}:1", sel.trim());
        checked += 1;
    }
    assert!(checked >= 8, "only {checked} colour rules checked");
}
