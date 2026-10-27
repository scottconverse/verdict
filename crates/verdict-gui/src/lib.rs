//! The Verdict page: the ledger and your decisions, on localhost (`vdt serve`).
//!
//! * It binds `127.0.0.1` only, never `0.0.0.0`, and refuses requests whose `Host` is not the
//!   loopback address it listens on (so a DNS-rebinding page cannot read it).
//! * It shows the sessions list, one session's report (the same text as `vdt report`) and the
//!   session's record log. All styling is embedded; nothing loads from the internet.
//! * It has exactly one write control: "Install hooks in this project", per host. Pressing it
//!   records a decision with writer `human` and shows the hook block to add. A press must come
//!   from this page: same `Origin` and a per-run token.
//!
//! Text is large and high contrast on purpose (body 20px, black on white).

mod decisions;
mod hooks;

pub use hooks::Host;

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdListener};
use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Form, Router,
    extract::{Path, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use verdict_core::part1c::{current_mode, one_line, resolve_session};
use verdict_core::{DecisionKind, Ledger, LedgerError, Record, Writer, list_sessions};

/// The default port for `vdt serve`.
pub const DEFAULT_PORT: u16 = 4717;

/// A page that is running in the background (used by tests and by `serve`).
#[derive(Debug, Clone)]
pub struct Page {
    /// The address it listens on. Always `127.0.0.1`.
    pub addr: SocketAddr,
    /// The per-run token the install button needs.
    pub token: String,
}

pub(crate) struct Shared {
    pub(crate) root: PathBuf,
    pub(crate) token: String,
    /// The folder that holds `.verdict/policy.json` (`None`: the user's home folder).
    pub(crate) home: Option<PathBuf>,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
}

pub(crate) type St = Arc<Shared>;

/// A fresh unguessable token for this run. Built from the operating system's randomness (the
/// randomly keyed hashers in std) mixed with the time and process id.
pub fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = blake3::Hasher::new();
    for _ in 0..4 {
        let mut s = std::collections::hash_map::RandomState::new().build_hasher();
        s.write_u64(0x0056_4552_4449_4354);
        h.update(&s.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.update(&nanos.to_le_bytes());
    h.update(&u64::from(std::process::id()).to_le_bytes());
    h.finalize().to_hex()[..40].to_string()
}

/// Bind the loopback address `127.0.0.1:port` (port 0 picks a free one). Never `0.0.0.0`.
pub fn bind(port: u16) -> std::io::Result<StdListener> {
    StdListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
}

fn shared(root: PathBuf, token: String, port: u16, home: Option<PathBuf>) -> St {
    Arc::new(Shared {
        root,
        token,
        home,
        allowed_hosts: vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")],
        allowed_origins: vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ],
    })
}

fn router(state: St) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/session/{id}", get(decisions::session_page))
        .route("/session/{id}/log", get(log_page))
        .route("/hooks/{host}", get(hooks_page))
        .route("/install-hooks", post(install_hooks))
        .route("/decide", post(decisions::decide))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn run_blocking(listener: StdListener, state: St) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        axum::serve(listener, router(state)).await
    })
}

/// Serve the page for the project at `root` on `127.0.0.1:port` until the process is stopped.
pub fn serve(root: PathBuf, port: u16) -> std::io::Result<()> {
    let listener = bind(port)?;
    let addr = listener.local_addr()?;
    let token = new_token();
    // A closed stdout (the caller stopped reading) must not stop the page: `println!` would
    // panic on a broken pipe, so write and ignore the error.
    {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        let _ = writeln!(out, "Verdict page: http://127.0.0.1:{}/", addr.port());
        let _ = writeln!(out, "Project: {}", root.display());
        let _ = writeln!(
            out,
            "Every button on the page records one of your decisions. Press Ctrl+C to stop."
        );
        let _ = out.flush();
    }
    run_blocking(listener, shared(root, token, addr.port(), None))
}

/// Start the page on a background thread and return where it listens. For tests.
pub fn start_background(root: PathBuf, port: u16) -> std::io::Result<Page> {
    start_background_with(root, port, None)
}

/// Like [`start_background`], with the folder that holds `.verdict/policy.json` (tests pass a
/// temp folder so they never read the real one).
pub fn start_background_with(
    root: PathBuf,
    port: u16,
    home: Option<PathBuf>,
) -> std::io::Result<Page> {
    let listener = bind(port)?;
    let addr = listener.local_addr()?;
    let token = new_token();
    let state = shared(root, token.clone(), addr.port(), home);
    std::thread::spawn(move || {
        let _ = run_blocking(listener, state);
    });
    Ok(Page { addr, token })
}

// ---------------------------------------------------------------- guard

pub(crate) fn deny(status: StatusCode, why: &str) -> Response {
    (
        status,
        Html(page("Refused", &format!("<p>{}</p>", esc(why)))),
    )
        .into_response()
}

async fn guard(State(st): State<St>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !st.allowed_hosts.iter().any(|h| h == host) {
        return deny(
            StatusCode::FORBIDDEN,
            "This page only answers requests addressed to its own localhost address.",
        );
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !st.allowed_origins.iter().any(|o| o == origin) {
            return deny(
                StatusCode::FORBIDDEN,
                "Refused: this request did not come from this page (its Origin is missing or different).",
            );
        }
        if let Some(site) = req.headers().get("sec-fetch-site")
            && site.to_str().unwrap_or("") != "same-origin"
        {
            return deny(
                StatusCode::FORBIDDEN,
                "Refused: the browser says this request came from another site.",
            );
        }
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; \
             frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

// ---------------------------------------------------------------- html

/// Escape text for HTML.
pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

const CSS: &str = "\
:root { color-scheme: light; }
* { box-sizing: border-box; }
body { margin: 0; background: #ffffff; color: #111111; font-family: system-ui, 'Segoe UI', Arial, sans-serif; font-size: 20px; line-height: 1.55; }
header { background: #111111; color: #ffffff; padding: 16px 28px; }
header a { color: #ffffff; background: #111111; font-size: 24px; font-weight: 700; text-decoration: none; }
header span { font-size: 20px; margin-left: 18px; color: #ffffff; background: #111111; }
main { max-width: 1100px; margin: 0 auto; padding: 24px 28px 64px; }
h1 { font-size: 34px; line-height: 1.25; margin: 8px 0 18px; }
h2 { font-size: 27px; line-height: 1.3; margin: 34px 0 12px; }
a { color: #0033a0; text-decoration: underline; font-weight: 600; }
a:focus, button:focus { outline: 4px solid #111111; outline-offset: 3px; }
p, li { font-size: 20px; }
.note { border: 3px solid #111111; padding: 12px 16px; background: #f2f2f2; margin: 14px 0; }
table { border-collapse: collapse; width: 100%; font-size: 19px; }
th, td { border: 2px solid #111111; padding: 10px 12px; text-align: left; vertical-align: top; overflow-wrap: anywhere; }
th { background: #e6e6e6; font-size: 19px; }
tr.refused td { background: #ffe0e0; font-weight: 700; }
pre { font-family: Consolas, 'Courier New', monospace; font-size: 18px; line-height: 1.5; white-space: pre-wrap; overflow-wrap: anywhere; border: 3px solid #111111; padding: 16px; background: #fafafa; color: #111111; margin: 12px 0; }
.verdict { font-size: 24px; font-weight: 700; border: 4px solid #111111; padding: 14px 16px; margin: 12px 0; background: #fff6cc; overflow-wrap: anywhere; }
button { font-family: inherit; font-size: 22px; font-weight: 700; background: #0033a0; color: #ffffff; border: 3px solid #111111; padding: 14px 22px; cursor: pointer; border-radius: 6px; }
button:hover { background: #001f63; color: #ffffff; }
.card { border: 3px solid #111111; padding: 16px 20px; margin: 16px 0; }
.card h3 { font-size: 25px; margin: 0 0 8px; }
.ok { font-weight: 700; }
.word { font-size: 40px; font-weight: 800; line-height: 1.2; border: 5px solid #111111; padding: 18px 22px; margin: 14px 0; background: #e6e6e6; color: #111111; overflow-wrap: anywhere; }
.word small { font-size: 22px; font-weight: 600; display: block; margin-top: 6px; }
.w-failed { background: #ffd0d0; border-color: #7a0000; }
.w-needs { background: #ffe9a8; }
.w-verified { background: #cdeccd; border-color: #0b4d0b; }
.w-partial { background: #ffdcb8; }
.w-unverified { background: #e0e0e0; }
input, select, textarea { font-family: inherit; font-size: 22px; color: #111111; background: #ffffff; border: 3px solid #111111; padding: 10px 12px; border-radius: 6px; max-width: 100%; }
textarea { width: 100%; min-height: 96px; }
label { font-size: 21px; font-weight: 700; display: block; margin: 10px 0 6px; }
button.danger { background: #8a0000; color: #ffffff; }
button.danger:hover { background: #5c0000; color: #ffffff; }
.fine { font-size: 19px; }
blockquote.claim { font-size: 24px; font-weight: 700; line-height: 1.45; color: #111111; background: #fff6cc; border-left: 8px solid #111111; margin: 10px 0 14px; padding: 12px 16px; overflow-wrap: anywhere; }
ul.names li { font-size: 24px; font-weight: 700; overflow-wrap: anywhere; }
ul.names code { font-family: Consolas, 'Courier New', monospace; font-size: 24px; color: #111111; }
";

pub(crate) fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{} - Verdict</title><style>{CSS}</style></head><body>\
<header><a href=\"/\">Verdict</a><span>the ledger and your decisions</span></header>\
<main><h1>{}</h1>{body}</main></body></html>",
        esc(title),
        esc(title)
    )
}

pub(crate) fn error_page(e: &LedgerError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Html(page(
            "Verdict could not read the ledger",
            &format!(
                "<p>{}</p><p>Nothing was changed. This is not a pass: the ledger could not be read.</p>",
                esc(&e.to_string())
            ),
        )),
    )
        .into_response()
}

pub(crate) fn not_found(what: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(page(
            "Not found",
            &format!(
                "<p>{}</p><p><a href=\"/\">Back to the sessions list</a></p>",
                esc(what)
            ),
        )),
    )
        .into_response()
}

// ---------------------------------------------------------------- pages

async fn home(State(st): State<St>) -> Response {
    let root = st.root.clone();
    let token = st.token.clone();
    match tokio::task::spawn_blocking(move || home_html(&root, &token)).await {
        Ok(Ok(html)) => Html(html).into_response(),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

fn home_html(root: &std::path::Path, token: &str) -> Result<String, LedgerError> {
    let ledger = Ledger::open_existing(root)?;
    let mut body = String::new();
    body.push_str(&format!(
        "<p>Project: <strong>{}</strong></p>",
        esc(&root.display().to_string())
    ));
    let mut hook_done: Vec<(Host, Option<u64>)> = Vec::new();
    match &ledger {
        None => {
            body.push_str(
                "<div class=\"note\">No ledger in this project yet, so there are no sessions. \
                 Nothing has been checked.</div>",
            );
            for h in Host::ALL {
                hook_done.push((h, None));
            }
        }
        Some(l) => {
            let integrity = l.verify()?;
            body.push_str(&format!(
                "<p>Ledger: <strong>{}</strong>. Mode: <strong>{}</strong>.</p>",
                esc(&integrity.summary()),
                esc(&current_mode(l)?)
            ));
            let mut sessions = list_sessions(l)?;
            sessions.reverse();
            body.push_str("<h2>Sessions</h2>");
            if sessions.is_empty() {
                body.push_str("<div class=\"note\">No sessions recorded yet.</div>");
            } else {
                body.push_str(
                    "<table><tr><th>Session</th><th>Started</th><th>Agent / model</th><th>Records</th></tr>",
                );
                for s in &sessions {
                    body.push_str(&format!(
                        "<tr><td><a href=\"/session/{id}\">{id}</a></td><td>{}</td><td>{} / {} [{}]</td><td>{}</td></tr>",
                        esc(&s.started_at),
                        esc(&s.agent),
                        esc(&s.model),
                        esc(&s.vendor),
                        s.record_count,
                        id = esc(&s.id)
                    ));
                }
                body.push_str("</table>");
            }
            for h in Host::ALL {
                let seq = l.records()?.iter().find_map(|s| match &s.record {
                    Record::Decision {
                        decision: DecisionKind::InstallHooks,
                        choice,
                        ..
                    } if choice == h.key() => Some(s.seq),
                    _ => None,
                });
                hook_done.push((h, seq));
            }
        }
    }
    body.push_str(
        "<h2>Hooks</h2><p>Hooks let Verdict see your agent's prompts and tool calls. \
         This button records your decision and shows you the block to add. It does not edit \
         any of your files.</p>",
    );
    for (h, seq) in hook_done {
        body.push_str(&format!("<div class=\"card\"><h3>{}</h3>", esc(h.title())));
        if let Some(seq) = seq {
            body.push_str(&format!(
                "<p class=\"ok\">You asked to install these hooks (decision record {seq}).</p>"
            ));
        }
        body.push_str(&format!(
            "<form method=\"post\" action=\"/install-hooks\">\
             <input type=\"hidden\" name=\"host\" value=\"{}\">\
             <input type=\"hidden\" name=\"token\" value=\"{}\">\
             <button type=\"submit\">Install hooks in this project</button></form>\
             <p><a href=\"/hooks/{}\">Show the {} hook block</a></p></div>",
            h.key(),
            esc(token),
            h.key(),
            esc(h.title())
        ));
    }
    Ok(page("This project's ledger", &body))
}

async fn log_page(State(st): State<St>, Path(id): Path<String>) -> Response {
    let root = st.root.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<String>, LedgerError> {
        let Some(l) = Ledger::open_existing(&root)? else {
            return Ok(None);
        };
        let Some(session) = resolve_session(&l, Some(&id)).ok().flatten() else {
            return Ok(None);
        };
        let records = l.records_for_session(&session)?;
        let mut body = format!(
            "<p><a href=\"/\">Back to the sessions list</a> &nbsp; \
             <a href=\"/session/{id}\">Report for this session</a></p>\
             <p>{} records, oldest first. Records are never changed or deleted.</p>\
             <table><tr><th>No.</th><th>Time</th><th>Written by</th><th>What</th></tr>",
            records.len(),
            id = esc(&id)
        );
        for s in &records {
            let class = if matches!(s.record, Record::RefusedWrite { .. }) {
                " class=\"refused\""
            } else {
                ""
            };
            body.push_str(&format!(
                "<tr{class}><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                s.seq,
                esc(&s.ts),
                esc(s.writer.as_str()),
                esc(&one_line(&s.record))
            ));
        }
        body.push_str("</table>");
        Ok(Some(page(&format!("Record log for session {id}"), &body)))
    })
    .await;
    match r {
        Ok(Ok(Some(html))) => Html(html).into_response(),
        Ok(Ok(None)) => not_found("There is no such session in this project's ledger."),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

fn block_html(h: Host) -> String {
    format!(
        "<p>Add this to <strong>{}</strong>. If that file already has a \"hooks\" section, merge \
         the three entries into it; do not replace your other hooks.</p><pre>{}</pre>\
         <p>This block is Verdict's best-known form of the hooks for {} and has not been \
         checked against a real {} session yet.</p>",
        esc(h.target_file()),
        esc(&h.block()),
        esc(h.title()),
        esc(h.title())
    )
}

async fn hooks_page(Path(host): Path<String>) -> Response {
    let Some(h) = Host::parse(&host) else {
        return not_found("That host is not one of the choices (claude_code, codex).");
    };
    Html(page(
        &format!("{} hook block", h.title()),
        &format!(
            "<p><a href=\"/\">Back to the sessions list</a></p>{}",
            block_html(h)
        ),
    ))
    .into_response()
}

#[derive(Deserialize)]
struct InstallForm {
    host: String,
    token: String,
}

async fn install_hooks(State(st): State<St>, Form(form): Form<InstallForm>) -> Response {
    if form.token != st.token {
        return deny(
            StatusCode::FORBIDDEN,
            "Refused: the page token is missing or wrong. Reload the page and press the button again.",
        );
    }
    let Some(h) = Host::parse(&form.host) else {
        return deny(StatusCode::BAD_REQUEST, "Unknown host.");
    };
    let root = st.root.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<(u64, Option<String>), LedgerError> {
        let mut l = Ledger::open(&root)?;
        let session = resolve_session(&l, None)?;
        let a = l.append(
            Writer::Human,
            session.as_deref(),
            Record::Decision {
                decision: DecisionKind::InstallHooks,
                choice: h.key().to_string(),
                reason: None,
                who: Writer::Human,
            },
        )?;
        Ok((a.seq, session))
    })
    .await;
    match r {
        Ok(Ok((seq, _))) => Html(page(
            &format!("Recorded: install {} hooks", h.title()),
            &format!(
                "<div class=\"note\"><strong>Your decision is recorded</strong> as record {seq} \
                 (written by: human). Verdict did not change any of your files.</div>{}\
                 <p><a href=\"/\">Back to the sessions list</a></p>",
                block_html(h)
            ),
        ))
        .into_response(),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}
