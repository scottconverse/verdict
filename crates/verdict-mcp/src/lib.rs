//! The Verdict MCP server (`vdt mcp`, stdio).
//!
//! Five tools, and none of them can write a finding, decision, verdict, mode switch or
//! commit-anyway. The agent writes claims, never verdicts:
//!
//! * `verdict_claim`: record one checkable line naming a URL, file or symbol (writer `agent`).
//! * `verdict_done_when`: for a vague prompt, how the human will know it is done. A condition that
//!   cannot be wrong is refused.
//! * `verdict_check`: runs the lane's checkers and desks now (the same fresh verification the Stop
//!   hook runs), then returns the verdict the core computes from the ledger (the agent cannot
//!   write or change it) and the same report `vdt report` prints. What has not run reads NOT
//!   ESTABLISHED, never clean.
//! * `verdict_scan`: the scan `vdt scan` runs (wiring, regressions and unfinished over the whole
//!   tree, no session, no claims). It writes nothing to the ledger; unread stacks read unverified.
//! * `verdict_decisions`: read only. Lists the human's open decisions.
//!
//! The server also recognizes the names an agent might try for a forbidden write (for example
//! `verdict_finding` or `verdict_commit_anyway`). They are not advertised, but if called they go
//! to the ledger as an agent write, which the ledger refuses and records as a `refused_write`.

use std::path::{Path, PathBuf};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};
use verdict_core::part1c::{
    check_claim_line, check_done_when, current_mode, open_decisions, resolve_session,
};
use verdict_core::{
    ClaimSource, DecisionKind, Finding, Ledger, LedgerError, Record, Severity, Tier, VerdictWord,
    Writer, render_report_with,
};

/// The tools this server advertises.
pub const TOOL_NAMES: [&str; 5] = [
    "verdict_claim",
    "verdict_done_when",
    "verdict_check",
    "verdict_scan",
    "verdict_decisions",
];

/// Names an agent might try in order to write what only the core, a checker or the human may
/// write. Never advertised. A call is turned into an agent write and refused by the ledger.
pub const FORBIDDEN_TOOL_NAMES: [&str; 8] = [
    "verdict_finding",
    "verdict_decision",
    "verdict_decide",
    "verdict_verdict",
    "verdict_set_mode",
    "verdict_mode_switch",
    "verdict_commit_anyway",
    "verdict_accept_finding",
];

/// What a tool call returns: text for the agent, and whether it is an error or refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The text the agent sees.
    pub text: String,
    /// True for a refusal or failure (the agent should not treat it as done).
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(text: impl Into<String>) -> Self {
        ToolOutcome {
            text: text.into(),
            is_error: false,
        }
    }
    fn refused(text: impl Into<String>) -> Self {
        ToolOutcome {
            text: text.into(),
            is_error: true,
        }
    }
}

/// The MCP server for one project.
#[derive(Debug, Clone)]
pub struct VerdictServer {
    root: PathBuf,
}

impl VerdictServer {
    /// A server for the project at `root`.
    pub fn new(root: PathBuf) -> Self {
        VerdictServer { root }
    }
}

fn schema(value: Value) -> std::sync::Arc<Map<String, Value>> {
    std::sync::Arc::new(value.as_object().cloned().unwrap_or_default())
}

/// The advertised tool definitions.
pub fn tool_definitions() -> Vec<Tool> {
    let session =
        json!({"type": "string", "description": "Session id. Default: the most recent session."});
    vec![
        Tool::new(
            "verdict_claim",
            "Record one claim: a single checkable line that names a URL, a file or a symbol. \
             Use it to write down what the prompt asked for, one line each. A line that names \
             none of those is refused. This records what you say is true; it does not prove it.",
            schema(json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "One checkable line."},
                    "names": {"type": "array", "items": {"type": "string"},
                        "description": "The URLs, files or symbols the line names. Each must appear in the line. Optional: they are read from the line when omitted."},
                    "source": {"type": "string",
                        "enum": ["prompt_translation", "plan_line", "final_message", "commit_message", "notes_file"],
                        "description": "Where the claim comes from. Default prompt_translation."},
                    "prompt_ordinal": {"type": "integer", "minimum": 1,
                        "description": "Which prompt it translates. Default: the latest prompt."},
                    "session": session.clone()
                },
                "required": ["text"]
            })),
        ),
        Tool::new(
            "verdict_done_when",
            "For a vague prompt, state how the human will know the work is done. It must be \
             something that can be wrong: name the page, file or command and what will be seen. \
             A condition like \"it works better\" is refused because it cannot be wrong.",
            schema(json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "One line: what the human will see when it is done."},
                    "session": session.clone()
                },
                "required": ["text"]
            })),
        ),
        Tool::new(
            "verdict_check",
            "Return Verdict's verdict and report for the session, the same text `vdt report` \
             prints. The verdict is computed by the core from the ledger; you cannot write or \
             change it. This tool does not run the checkers, so anything that has not run is \
             listed under NOT ESTABLISHED. It says clean only when the ledger proves it.",
            schema(json!({"type": "object", "properties": {"session": session.clone()}})),
        ),
        Tool::new(
            "verdict_scan",
            "Scan the whole project, with no session and no claims, and return the report `vdt scan` \
             prints: every broken call, every route or command nothing calls, every control with no \
             handler, every called stub, unfinished work and regressions, and a count of CONNECTED, \
             BROKEN, NOT BUILT, HALF BUILT and SKIPPED. Code in a stack it does not read is listed \
             as unverified, never clean. It writes nothing to the ledger and is not a verdict.",
            schema(json!({"type": "object", "properties": {}})),
        ),
        Tool::new(
            "verdict_decisions",
            "List the human's open decisions (read only). You can read them; you can never answer \
             them. Only the human answers, on the Verdict page.",
            schema(json!({"type": "object", "properties": {"session": session}})),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
    ]
}

fn str_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn open_ledger(root: &Path) -> Result<Ledger, LedgerError> {
    Ledger::open(root)
}

fn err_text(e: &LedgerError) -> String {
    format!("Verdict could not complete this: {e}. Nothing was recorded as checked.")
}

/// Run one tool call against the project at `root`. The heart of the server, usable without MCP.
pub fn handle_tool(root: &Path, name: &str, args: &Map<String, Value>) -> Option<ToolOutcome> {
    let out = match name {
        "verdict_claim" => claim(root, args),
        "verdict_done_when" => done_when(root, args),
        "verdict_check" => check_tool(root, args),
        "verdict_scan" => scan_tool(root),
        "verdict_decisions" => decisions(root, args),
        n if FORBIDDEN_TOOL_NAMES.contains(&n) => forbidden(root, n, args),
        _ => return None,
    };
    Some(out.unwrap_or_else(|e| ToolOutcome::refused(err_text(&e))))
}

const NO_SESSION: &str = "No session is open in this project yet, so there is nothing to attach \
this to. The hook adapter opens a session at the first prompt; nothing was recorded.";

fn claim(root: &Path, args: &Map<String, Value>) -> Result<ToolOutcome, LedgerError> {
    let Some(text) = str_arg(args, "text") else {
        return Ok(ToolOutcome::refused(
            "`text` is required: one checkable line.",
        ));
    };
    let given: Vec<String> = match args.get("names") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(_) => {
            return Ok(ToolOutcome::refused("`names` must be a list of strings."));
        }
    };
    let names = match check_claim_line(text, &given) {
        Ok(n) => n,
        Err(reason) => return Ok(ToolOutcome::refused(format!("Claim refused: {reason}"))),
    };
    let source = match str_arg(args, "source").unwrap_or("prompt_translation") {
        "prompt_translation" => ClaimSource::PromptTranslation,
        "plan_line" => ClaimSource::PlanLine,
        "final_message" => ClaimSource::FinalMessage,
        "commit_message" => ClaimSource::CommitMessage,
        "notes_file" => ClaimSource::NotesFile,
        other => {
            return Ok(ToolOutcome::refused(format!(
                "Claim refused: unknown source {other:?}. Use verdict_done_when for done-when lines."
            )));
        }
    };
    let mut ledger = open_ledger(root)?;
    let Some(session) = resolve_session(&ledger, str_arg(args, "session"))? else {
        return Ok(ToolOutcome::refused(NO_SESSION));
    };
    let prompt_ordinal = match args.get("prompt_ordinal").and_then(Value::as_u64) {
        Some(n) => Some(n as u32),
        None if source == ClaimSource::PromptTranslation => ledger
            .records_for_session(&session)?
            .iter()
            .rev()
            .find_map(|s| match &s.record {
                Record::Prompt { ordinal, .. } => Some(*ordinal),
                _ => None,
            }),
        None => None,
    };
    let appended = ledger.append(
        Writer::Agent,
        Some(&session),
        Record::Claim {
            source,
            // The ledger is append-only, so a secret written here could never be taken out
            // again. The hook path redacts; so does this one.
            text: verdict_adapters::redact::redact(text.trim()),
            names: names
                .iter()
                .map(|n| verdict_adapters::redact::redact(n))
                .collect(),
            prompt_ordinal,
        },
    )?;
    Ok(ToolOutcome::ok(format!(
        "Recorded claim as record {} in session {session}. It names: {}. This is your claim, not \
         a finding: nothing has been checked.",
        appended.seq,
        names.join(", ")
    )))
}

fn done_when(root: &Path, args: &Map<String, Value>) -> Result<ToolOutcome, LedgerError> {
    let Some(text) = str_arg(args, "text") else {
        return Ok(ToolOutcome::refused("`text` is required: one line."));
    };
    let names = match check_done_when(text) {
        Ok(n) => n,
        Err(reason) => {
            return Ok(ToolOutcome::refused(format!("Done-when refused: {reason}")));
        }
    };
    let mut ledger = open_ledger(root)?;
    let Some(session) = resolve_session(&ledger, str_arg(args, "session"))? else {
        return Ok(ToolOutcome::refused(NO_SESSION));
    };
    let appended = ledger.append(
        Writer::Agent,
        Some(&session),
        Record::Claim {
            source: ClaimSource::DoneWhen,
            text: verdict_adapters::redact::redact(text.trim()),
            names,
            prompt_ordinal: None,
        },
    )?;
    Ok(ToolOutcome::ok(format!(
        "Recorded done-when as record {} in session {session}. The human can now judge it; nothing \
         has been checked.",
        appended.seq
    )))
}

/// `verdict_check`: run the lane's checkers and desks now, then return the verdict the core
/// computed from the ledger and the report. The agent cannot write or change a verdict; the fresh
/// run is the ordinary completion path, not an optional extra.
fn check_tool(root: &Path, args: &Map<String, Value>) -> Result<ToolOutcome, LedgerError> {
    let Some(mut ledger) = Ledger::open_existing(root)? else {
        return Ok(ToolOutcome::ok(
            "NOT ESTABLISHED. verdict_check: nothing was checked. There is no ledger in this project yet, \
             so there is no session to report on. This is not a pass.",
        ));
    };
    let Some(session) = resolve_session(&ledger, str_arg(args, "session"))? else {
        return Ok(ToolOutcome::ok(
            "NOT ESTABLISHED. verdict_check: nothing was checked. This project's ledger has no session yet. \
             This is not a pass.",
        ));
    };
    let verified = match verdict_engine::verify(
        root,
        &mut ledger,
        &session,
        None,
        &verdict_engine::VerifyOptions {
            budget: None,
            reason: Some("verdict_check".into()),
        },
    ) {
        Ok(v) => v,
        Err(reason) => {
            return Ok(ToolOutcome::refused(format!(
                "Verdict could not run the checks: {reason}. The failure was recorded; this is not a pass."
            )));
        }
    };
    let computed = verified.computed;
    let report = render_report_with(&ledger, Some(&session), Some(&computed))?;
    Ok(ToolOutcome::ok(format!(
        "verdict_check: ran the lane's checkers and desks now (gate result record {}), then computed the \
         verdict from the ledger. The verdict is {}. An agent cannot write or change a verdict. Anything \
         that has not been checked is listed under NOT ESTABLISHED. This is the same report `vdt report` \
         prints:\n\n{report}",
        verified.gate_seq,
        computed.label()
    )))
}

/// `verdict_scan`: the same scan `vdt scan` runs (wiring, regressions and unfinished over the
/// whole tree, no session, no claims). It reads the code and writes nothing: no ledger record,
/// no file. A scan is not a verdict.
fn scan_tool(root: &Path) -> Result<ToolOutcome, LedgerError> {
    let report = verdict_checkers::scan::scan(root, None).render();
    Ok(ToolOutcome::ok(format!(
        "verdict_scan: read the whole project with no session and no claims. Nothing was written to \
         the ledger, and this is not a verdict: only the core computes one. This is the same report \
         `vdt scan` prints:\n\n{report}"
    )))
}

fn decisions(root: &Path, args: &Map<String, Value>) -> Result<ToolOutcome, LedgerError> {
    let Some(ledger) = Ledger::open_existing(root)? else {
        return Ok(ToolOutcome::ok(
            "No ledger in this project yet, so no decisions are recorded. Mode: shadow (default). \
             Only the human can answer decisions, on the Verdict page.",
        ));
    };
    let session = resolve_session(&ledger, str_arg(args, "session"))?;
    let open = open_decisions(&ledger, session.as_deref())?;
    let mode = current_mode(&ledger)?;
    let mut out = format!(
        "Mode: {mode}. Only the human can change it. You can read the decisions below; you cannot \
         answer them (an attempt is refused and recorded).\n\nOPEN DECISIONS ({})\n",
        open.len()
    );
    if open.is_empty() {
        out.push_str("  (none)\n");
    }
    for (i, d) in open.iter().enumerate() {
        out.push_str(&format!(
            "  {}. [{}] {}\n     if unanswered: {}\n",
            i + 1,
            d.decision.as_str(),
            d.question,
            d.default_if_unanswered
        ));
    }
    Ok(ToolOutcome::ok(out))
}

/// An agent reached for a write only the core, a checker or the human may make. Pass it to the
/// ledger as an agent write; the ledger refuses and records it.
fn forbidden(
    root: &Path,
    name: &str,
    args: &Map<String, Value>,
) -> Result<ToolOutcome, LedgerError> {
    let mut ledger = open_ledger(root)?;
    let session = resolve_session(&ledger, str_arg(args, "session")).unwrap_or(None);
    let text = |k: &str, d: &str| str_arg(args, k).unwrap_or(d).to_string();
    let record = match name {
        "verdict_finding" => Record::Finding(Finding {
            id: text("id", "agent-attempt"),
            severity: Severity::Major,
            title: text("title", "(finding written by an agent)"),
            claim_seq: None,
            evidence: vec![],
            tier: Tier::Static,
            blast: None,
            judgment: false,
            fix_hint: None,
        }),
        "verdict_verdict" => Record::Verdict {
            word: VerdictWord::Verified,
            highest_tier: Some(Tier::Static),
            counts_by_tier: Default::default(),
            not_established: vec![],
            policy_version: 0,
            lane: None,
        },
        _ => {
            let (decision, default_choice) = match name {
                "verdict_commit_anyway" => (DecisionKind::CommitAnyway, "commit_anyway"),
                "verdict_set_mode" | "verdict_mode_switch" => (DecisionKind::ModeSwitch, "enforce"),
                "verdict_accept_finding" => (DecisionKind::AcceptFindingNotReal, "accepted"),
                _ => (DecisionKind::AcceptFindingNotReal, "accepted"),
            };
            Record::Decision {
                decision,
                choice: text("choice", default_choice),
                reason: str_arg(args, "reason").map(str::to_string),
                // An agent cannot claim to be the human: the ledger refuses on the writer
                // before it looks at this field.
                who: Writer::Human,
            }
        }
    };
    match ledger.append(Writer::Agent, session.as_deref(), record) {
        Err(LedgerError::Refused {
            refused_seq,
            reason,
        }) => Ok(ToolOutcome::refused(format!(
            "REFUSED: {reason}. The attempt was recorded as record {refused_seq} (refused_write) \
             and will show in the report. Agents write claims, never verdicts."
        ))),
        Err(e) => Err(e),
        Ok(_) => Ok(ToolOutcome::refused(
            "REFUSED. (The ledger unexpectedly accepted this; it was not meant to be possible.)",
        )),
    }
}

impl ServerHandler for VerdictServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let args = request.arguments.unwrap_or_default();
        let root = self.root.clone();
        let outcome = tokio::task::spawn_blocking(move || handle_tool(&root, &name, &args))
            .await
            .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?;
        let Some(outcome) = outcome else {
            return Err(McpError::invalid_params(
                format!("unknown tool {:?}", request.name),
                None,
            ));
        };
        let block = ContentBlock::text(outcome.text);
        Ok(if outcome.is_error {
            CallToolResult::error(vec![block])
        } else {
            CallToolResult::success(vec![block])
        }
        .into())
    }
}

/// Serve MCP over stdio for the project at `root` until the client closes the pipe.
pub fn serve_stdio(root: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = VerdictServer::new(root)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
