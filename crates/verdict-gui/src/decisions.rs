//! The session page with the verdict at the top and the decisions queue, and the one route
//! (`POST /decide`) that answers them.
//!
//! Every button here records exactly one decision as writer `human`, through the same
//! Host/Origin/token guard as the hooks button (the guard is a layer on the whole router). A
//! request without this run's token writes nothing. The validation and the record itself live in
//! `verdict_core::human`, shared with `vdt`, so the page and the command line cannot disagree.

use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::path::Path as FsPath;
use verdict_core::exec_approval::{
    ApprovalChoice, commands_hash, describe_commands, parse_choice, policy_timeout_text,
};
use verdict_core::human::{Action, active_pause, apply, chosen_mode, lane_from_text};
use verdict_core::lane::{ALL_LANES, lane_name};
use verdict_core::part1c::{one_line, open_decisions, resolve_session};
use verdict_core::report::render_report_with;
use verdict_core::timefmt::format_unix_millis as format_millis;
use verdict_core::verdict::{analyze, compute, finding_view_at, live_input, word_label};
use verdict_core::{DecisionKind, Ledger, LedgerError, Record, VerdictWord, Writer, list_sessions};

use crate::{St, deny, error_page, esc, not_found, page};

#[derive(Deserialize, Default)]
pub(crate) struct RecordedQuery {
    recorded: Option<u64>,
    ran: Option<u64>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn word_class(w: VerdictWord) -> &'static str {
    match w {
        VerdictWord::Failed => "w-failed",
        VerdictWord::NeedsYou => "w-needs",
        VerdictWord::Verified | VerdictWord::ClearToShip => "w-verified",
        VerdictWord::Partial => "w-partial",
        VerdictWord::Unverified => "w-unverified",
    }
}

fn word_gloss(w: VerdictWord) -> &'static str {
    match w {
        VerdictWord::Failed => {
            "Something is proven wrong. Fix it, or tell Verdict below that it is not real."
        }
        VerdictWord::NeedsYou => {
            "Nothing is proven wrong, but something is yours to settle. See below."
        }
        VerdictWord::Verified => {
            "Checked, and nothing wrong was found. The word in brackets says how: static means the code was only read, ran means your tests were run, browsed means a page was loaded."
        }
        VerdictWord::ClearToShip => {
            "Every release check passed and nothing is open. Tagging the release is still yours."
        }
        VerdictWord::Partial => "A check this lane needs did not run, so this is not a pass.",
        VerdictWord::Unverified => "Nothing was checked. This is not a pass.",
    }
}

/// One form that records one decision. Nothing is shown to the browser that is not escaped.
fn form(
    token: &str,
    session: &str,
    action: &str,
    inner: &str,
    button: &str,
    danger: bool,
) -> String {
    format!(
        "<form method=\"post\" action=\"/decide\">\
         <input type=\"hidden\" name=\"token\" value=\"{}\">\
         <input type=\"hidden\" name=\"session\" value=\"{}\">\
         <input type=\"hidden\" name=\"action\" value=\"{}\">{inner}\
         <button type=\"submit\"{}>{}</button></form>",
        esc(token),
        esc(session),
        esc(action),
        if danger { " class=\"danger\"" } else { "" },
        esc(button)
    )
}

fn card(title: &str, body: &str) -> String {
    format!("<div class=\"card\"><h3>{title}</h3>{body}</div>")
}

fn session_html(
    root: &FsPath,
    home: Option<&FsPath>,
    token: &str,
    id: &str,
    recorded: Option<u64>,
    ran: Option<u64>,
) -> Result<Option<String>, LedgerError> {
    let Some(l) = Ledger::open_existing(root)? else {
        return Ok(None);
    };
    if !list_sessions(&l)?.iter().any(|s| s.id == id) {
        return Ok(None);
    }
    let live = live_input(root, &l, id, home)?;
    let c = compute(&l, id, &live.input)?;
    let report = render_report_with(&l, Some(id), Some(&c))?;
    let a = analyze(&l, id)?;
    let mode = chosen_mode(&l)?.unwrap_or_else(|| live.policy.gate_mode().to_string());
    let pause = active_pause(&l, now_ms())?;

    let mut body = format!(
        "<p><a href=\"/\">Back to the sessions list</a> &nbsp; \
         <a href=\"/session/{id}/log\">Record log for this session</a> &nbsp; \
         <a href=\"/session/{id}/packet\">Download the acceptance packet</a> &nbsp; \
         <a href=\"/session/{id}/sarif\">Download SARIF</a> &nbsp; \
         <a href=\"/decisions.jsonl\">Download decisions.jsonl</a></p>",
        id = esc(id)
    );
    // Running the checks is a page action (it writes checks and a verdict, never a decision).
    body.push_str(&format!(
        "<div class=\"card\"><h3>Run the checks now</h3>\
         <p>The Stop hook and the agent's verdict_check already run the lane's checkers and desks. \
         This button runs them again on the code as it stands, then recomputes the verdict.</p>\
         <form method=\"post\" action=\"/session/{id}/check\" style=\"display:inline\">\
         <input type=\"hidden\" name=\"token\" value=\"{token}\">\
         <button type=\"submit\">Run the checkers now</button></form> \
         <form method=\"post\" action=\"/session/{id}/desks\" style=\"display:inline\">\
         <input type=\"hidden\" name=\"token\" value=\"{token}\">\
         <button type=\"submit\">Run the desks now</button></form></div>",
        id = esc(id),
        token = esc(token)
    ));
    if let Some(seq) = ran {
        body.push_str(&format!(
            "<div class=\"note\">You ran the checks. The gate result is record {seq}; the verdict \
             below was worked out again from the ledger.</div>"
        ));
    }

    if let Some(seq) = recorded
        && let Some(s) = l.get(seq)?
        && s.writer == Writer::Human
        && matches!(s.record, Record::Decision { .. })
    {
        body.push_str(&format!(
            "<div class=\"note\"><strong>Your decision is recorded</strong> as record {seq} \
             (written by: human): {}. The verdict below was worked out again from the ledger.</div>",
            esc(&one_line(&s.record))
        ));
    }

    // The verdict, first and large.
    body.push_str(&format!(
        "<div class=\"word {}\">VERDICT: {}<small>{}</small></div>",
        word_class(c.word),
        esc(&word_label(c.word, c.highest_tier)),
        esc(word_gloss(c.word))
    ));
    body.push_str(&format!(
        "<p>Lane: <strong>{}</strong>{}. Gate mode: <strong>{}</strong>.{}</p>",
        lane_name(c.lane),
        if c.lane != c.implied_lane {
            format!(
                " (you raised it; what the changes touch implies {})",
                lane_name(c.implied_lane)
            )
        } else {
            format!(
                " (from what the changes touch: {})",
                esc(c.lane_reasons.first().map_or("nothing", String::as_str))
            )
        },
        esc(&mode),
        match &pause {
            Some(p) => format!(
                " The gate is <strong>paused</strong> until {}.",
                esc(&format_millis(p.until_ms))
            ),
            None => String::new(),
        }
    ));
    if let Some(w) = &live.policy_warning {
        body.push_str(&format!(
            "<div class=\"note\">The policy file could not be used, so the built-in default was used instead: {}</div>",
            esc(w)
        ));
    }

    // ---- what is waiting for you
    body.push_str("<h2>Waiting for you</h2>");
    let mut waiting = 0;
    let mut open: Vec<_> = a.findings.iter().filter(|f| f.is_open()).collect();
    open.sort_by_key(|f| !c.fix_first.iter().any(|x| x.id == f.finding.id));
    for f in open {
        waiting += 1;
        let blocking = c.fix_first.iter().any(|x| x.id == f.finding.id);
        let ev = f
            .finding
            .evidence
            .iter()
            .map(|e| {
                let mut text = e.describe();
                if let verdict_core::PointerStatus::Stale(why) = e.check(root) {
                    text.push_str(&format!(" (STALE: {why})"));
                }
                esc(&text)
            })
            .collect::<Vec<_>>()
            .join(", ");
        body.push_str(&card(
            &format!(
                "{} [{:?}] {}",
                if blocking { "Proven wrong:" } else { "To settle:" },
                f.finding.severity,
                esc(&f.finding.title)
            ),
            &format!(
                "<p>Found by a check at the <strong>{}</strong> tier. Evidence: {}. (record {}, finding {})</p>\
                 <p>If this is a false alarm, tell Verdict. That closes this one finding and nothing else.</p>{}",
                esc(f.finding.tier.as_str()),
                if ev.is_empty() { "none".to_string() } else { ev },
                f.seq,
                esc(&f.finding.id),
                form(
                    token,
                    id,
                    "accept_finding",
                    &format!("<input type=\"hidden\" name=\"value\" value=\"{}\">", f.seq),
                    "Accept as not real",
                    false
                )
            ),
        ));
    }
    // A commit-anyway override from another session stays open until its finding is settled; the
    // finding lives in that session, so its button is here too.
    let all = l.records()?;
    let mut shown: Vec<u64> = Vec::new();
    for o in a
        .overrides
        .iter()
        .filter(|o| o.open && o.session.as_deref().is_some_and(|s| s != id))
    {
        for fid in &o.finding_ids {
            let Some(last) = all
                .iter()
                .rev()
                .find(|s| matches!(&s.record, Record::Finding(f) if f.id == *fid))
            else {
                continue;
            };
            let Some(v) = finding_view_at(&all, last.seq).filter(|v| v.is_open()) else {
                continue;
            };
            if shown.contains(&v.seq) {
                continue;
            }
            shown.push(v.seq);
            waiting += 1;
            body.push_str(&card(
                &format!(
                    "Still open from session {}: {}",
                    esc(o.session.as_deref().unwrap_or("")),
                    esc(&v.finding.title)
                ),
                &format!(
                    "<p>You committed past this in session {} (record {}, your reason: \"{}\"). \
                     Until the finding is fixed, or you tell Verdict it is not real, no session can \
                     reach CLEAR TO SHIP. (record {}, finding {})</p>{}",
                    esc(o.session.as_deref().unwrap_or("")),
                    o.seq,
                    esc(&o.reason),
                    v.seq,
                    esc(&v.finding.id),
                    form(
                        token,
                        id,
                        "accept_finding",
                        &format!("<input type=\"hidden\" name=\"value\" value=\"{}\">", v.seq),
                        "Accept as not real",
                        false
                    )
                ),
            ));
        }
    }
    // The latest answer about running the repo's commands. An approval covers the exact commands,
    // tiers, shell and timeout it was given for; if the policy changed since, it is asked again.
    let mut commands_state = None;
    for s in l.records()? {
        if let Record::Decision {
            decision: DecisionKind::RunProjectCommands,
            choice,
            who: Writer::Human,
            ..
        } = &s.record
        {
            commands_state = Some(parse_choice(choice));
        }
    }
    let current_hash = commands_hash(&live.policy);
    let approval_current =
        matches!(&commands_state, Some(ApprovalChoice::Approved(h)) if *h == current_hash);
    let commands_refused = matches!(&commands_state, Some(ApprovalChoice::Refused));
    if c.required.iter().any(|n| n == "exec") && !approval_current && !commands_refused {
        waiting += 1;
        let why_again = match &commands_state {
            Some(ApprovalChoice::Approved(_)) => {
                "<p><strong>The commands changed since you approved them.</strong> Your earlier \
                 approval no longer counts. Read the list again.</p>"
            }
            Some(ApprovalChoice::Unbound) => {
                "<p>Your earlier approval did not record which commands it covered, so it counts \
                 for none. Read the list and approve again.</p>"
            }
            _ => "",
        };
        let listed = describe_commands(&live.policy);
        let list_html = if live.policy_warning.is_some() {
            "<p><strong>The policy file could not be read, so there are no commands to approve \
             yet. Fix the policy file first.</strong></p>"
                .to_string()
        } else if listed.is_empty() {
            "<p><strong>The policy lists no commands.</strong></p>".to_string()
        } else {
            format!(
                "<p><strong>These are the commands you would be approving. Verdict will run \
                 exactly these, on this computer, in this folder:</strong></p>\
                 <pre style=\"font-size:26px;font-weight:700;line-height:1.6\">{}</pre>\
                 <p>Time limit for each: {}.</p>",
                listed.iter().map(|x| esc(x)).collect::<Vec<_>>().join("\n"),
                esc(&policy_timeout_text(&live.policy).unwrap_or_else(|| "the default".into()))
            )
        };
        let button = if live.policy_warning.is_some() || listed.is_empty() {
            String::new()
        } else {
            form(
                token,
                id,
                "approve_commands",
                "",
                "Approve this repo's commands",
                false,
            )
        };
        body.push_str(&card(
            "Run this repo's own commands?",
            &format!(
                "<p>The build, lint and test commands listed in this project's policy are run to get a \
                 <strong>ran</strong> result. Verdict runs none that the policy does not list. Until you \
                 approve, that check reads as not run. If the list changes later, you are asked again.</p>\
                 {why_again}{list_html}{button}"
            ),
        ));
    }
    for d in open_decisions(&l, Some(id))? {
        match d.decision {
            DecisionKind::InstallHooks => {
                waiting += 1;
                // The card is a button: pressing it records the human's decision and shows the
                // block to add. The link to the sessions page stays, for the block itself.
                let host = d
                    .key
                    .split_once(':')
                    .map(|(_, h)| h.to_string())
                    .unwrap_or_default();
                let host_link = esc(&host);
                body.push_str(&card(
                    &esc(&d.question),
                    &format!(
                        "<p>This records your decision and shows you the exact block to add. It \
                         does not edit any of your files.</p>\
                         <form method=\"post\" action=\"/install-hooks\">\
                         <input type=\"hidden\" name=\"host\" value=\"{host_link}\">\
                         <input type=\"hidden\" name=\"token\" value=\"{}\">\
                         <button type=\"submit\">Install these hooks</button></form>\
                         <p>Answer it on the <a href=\"/\">sessions page</a>, under Hooks.</p>",
                        esc(token)
                    ),
                ));
            }
            DecisionKind::AcceptPromptTranslation => {
                let Some(seq) = d.record_seq else { continue };
                waiting += 1;
                let names = d
                    .added_names
                    .iter()
                    .map(|n| format!("<li><code>{}</code></li>", esc(n)))
                    .collect::<String>();
                body.push_str(&card(
                    "A claim names something your prompt did not",
                    &format!(
                        "<p>The agent wrote down what it thinks you asked for:</p>\
                         <blockquote class=\"claim\">{}</blockquote>\
                         <p>It names these, which your prompt did not mention:</p>\
                         <ul class=\"names\">{names}</ul>\
                         <p>If that is what you asked for, accept it. Until you do, the verdict stays \
                         at NEEDS YOU. (claim record {seq})</p>{}",
                        esc(d.claim_text.as_deref().unwrap_or(&d.question)),
                        form(
                            token,
                            id,
                            "accept_translation",
                            &format!("<input type=\"hidden\" name=\"value\" value=\"{seq}\">"),
                            "Accept this claim as what I asked for",
                            false
                        )
                    ),
                ));
            }
            _ => {}
        }
    }
    // The plan is the human's to name when the policy lists more than one candidate.
    let plans: Vec<String> = live
        .policy
        .value
        .pointer("/plans")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let plan_chosen = l.records()?.iter().any(|s| {
        matches!(
            &s.record,
            Record::Decision {
                decision: DecisionKind::PlanFile,
                who: Writer::Human,
                ..
            }
        )
    });
    if plans.len() > 1 && !plan_chosen {
        waiting += 1;
        let options: String = plans
            .iter()
            .map(|p| format!("<option value=\"{}\">{}</option>", esc(p), esc(p)))
            .collect();
        body.push_str(&card(
            "Which file is the plan?",
            &format!(
                "<p>More than one plan candidate is configured. Pick the one this session was \
                 working from, or leave it: the checks run plan-less and say so.</p>{}",
                form(
                    token,
                    id,
                    "plan_file",
                    &format!(
                        "<label for=\"plan\">The plan file</label><select id=\"plan\" name=\"value\">{options}</select>"
                    ),
                    "Use this plan",
                    false
                )
            ),
        ));
    }
    // Withdraw a prompt the human cancelled.
    let prompts: Vec<(u32, String)> = l
        .records_for_session(id)?
        .into_iter()
        .filter_map(|s| match s.record {
            Record::Prompt { ordinal, text, .. } => Some((ordinal, text)),
            _ => None,
        })
        .collect();
    if let Some((ordinal, text)) = prompts.last() {
        waiting += 1;
        body.push_str(&card(
            "Withdraw a prompt you cancelled",
            &format!(
                "<p>The latest prompt of this session is #{ordinal}:</p>\
                 <blockquote class=\"claim\">{}</blockquote>\
                 <p>Withdrawing it records that you cancelled it; it stays in the ledger and stays \
                 out of what is still asked for.</p>{}",
                esc(text),
                form(
                    token,
                    id,
                    "withdraw_prompt",
                    &format!("<input type=\"hidden\" name=\"value\" value=\"{ordinal}\">"),
                    "Withdraw this prompt",
                    false
                )
            ),
        ));
    }
    // Waive words no check can cover.
    body.push_str(&card(
        "Waive words no check can cover",
        &format!(
            "<p>If a prompt asked for something like \"faster\" or \"nicer\", no check can settle \
             it. Naming the words here records your decision; they stop blocking the report.</p>{}",
            form(
                token,
                id,
                "waive_words",
                "<label for=\"words\">Words to waive</label><input id=\"words\" name=\"value\" size=\"40\" placeholder=\"faster, nicer\">",
                "Waive these words",
                false
            )
        ),
    ));
    // Register the MCP server (human only, like the hooks).
    let mcp_answered: Vec<String> = l
        .records()?
        .into_iter()
        .filter_map(|s| match &s.record {
            Record::Decision {
                decision: DecisionKind::McpInstall,
                choice,
                who: Writer::Human,
                ..
            } => Some(choice.clone()),
            _ => None,
        })
        .collect();
    let mut mcp_buttons = String::new();
    for host in ["claude", "cursor", "codex", "dsh", "antigravity"] {
        if mcp_answered.iter().any(|h| h == host) {
            mcp_buttons.push_str(&format!("<p class=\"ok\">Registered with {host}.</p>"));
        } else {
            mcp_buttons.push_str(&format!(
                "<form method=\"post\" action=\"/mcp-install\" style=\"display:inline\">\
                 <input type=\"hidden\" name=\"host\" value=\"{host}\">\
                 <input type=\"hidden\" name=\"session\" value=\"{}\">\
                 <input type=\"hidden\" name=\"token\" value=\"{}\">\
                 <button type=\"submit\">Register with {host}</button></form> ",
                esc(id),
                esc(token)
            ));
        }
    }
    waiting += 1;
    body.push_str(&card(
        "Register Verdict's MCP server",
        &format!(
            "<p>The agent reads the ledger and records claims through this server. Registering it \
             writes the host's own config file; press a button below when you want that.</p>{mcp_buttons}"
        ),
    ));

    if waiting == 0 {
        body.push_str("<div class=\"note\">Nothing is waiting for you.</div>");
    }

    // ---- controls
    body.push_str("<h2>Your controls</h2>");
    let higher: Vec<_> = ALL_LANES.into_iter().filter(|l| *l > c.lane).collect();
    if higher.is_empty() {
        body.push_str(&card(
            "Raise the lane",
            "<p>The lane is already the highest.</p>",
        ));
    } else {
        let options: String = higher
            .iter()
            .map(|l| format!("<option value=\"{0}\">{0}</option>", lane_name(*l)))
            .collect();
        body.push_str(&card(
            "Raise the lane",
            &format!(
                "<p>More checks run in a higher lane. You can raise it; nothing can lower it below what the changes touch.</p>{}",
                form(
                    token,
                    id,
                    "raise_lane",
                    &format!("<label for=\"lane\">Raise it to</label><select id=\"lane\" name=\"value\">{options}</select>"),
                    "Raise the lane",
                    false
                )
            ),
        ));
    }
    let other_mode = if mode == "enforce" {
        "shadow"
    } else {
        "enforce"
    };
    body.push_str(&card(
        &format!("Gate mode: {}", esc(&mode)),
        &format!(
            "<p>Shadow never holds a commit; it only records what it would have done. Enforce holds a \
             commit whose verdict is FAILED, until you commit anyway or fix it.</p>{}",
            form(
                token,
                id,
                "set_mode",
                &format!("<input type=\"hidden\" name=\"value\" value=\"{other_mode}\">"),
                &format!("Switch to {other_mode}"),
                false
            )
        ),
    ));
    body.push_str(&card(
        "Pause the gate",
        &format!(
            "<p>The gate does nothing until the time you give is over, and records that it was paused.</p>{}",
            form(
                token,
                id,
                "pause",
                "<label for=\"dur\">Pause for (for example 30m, 2h, 1d)</label><input id=\"dur\" name=\"value\" value=\"30m\" size=\"8\">",
                "Pause the gate",
                false
            )
        ),
    ));
    if c.word == VerdictWord::Failed {
        body.push_str(&card(
            "Commit anyway",
            &format!(
                "<p>This lets the commit that is staged right now go through even though the verdict is \
                 FAILED. <strong>The verdict stays FAILED</strong>, and your reason is kept beside it. \
                 You must type a reason.</p>{}",
                form(
                    token,
                    id,
                    "commit_anyway",
                    "<label for=\"why\">Why are you committing past this?</label><textarea id=\"why\" name=\"reason\"></textarea>",
                    "Commit anyway",
                    true
                )
            ),
        ));
    } else {
        body.push_str(&card(
            "Commit anyway",
            "<p>This appears here when the verdict is FAILED and a commit is held.</p>",
        ));
    }

    // ---- claims, evidence and unplanned work (the report repeats the settled form)
    body.push_str("<h2>Claims and evidence</h2>");
    let claims: Vec<_> = l
        .records_for_session(id)?
        .into_iter()
        .filter(|s| matches!(s.record, Record::Claim { .. }))
        .collect();
    if claims.is_empty() {
        body.push_str("<p>No claims were recorded for this session.</p>");
    } else {
        body.push_str(
            "<table><tr><th>No.</th><th>Source</th><th>Claim</th><th>What it names</th>\
             <th>Findings about it</th></tr>",
        );
        for s in claims {
            let Record::Claim {
                source,
                text,
                names,
                ..
            } = &s.record
            else {
                continue;
            };
            let borne_on: Vec<String> = a
                .findings
                .iter()
                .filter(|f| f.finding.claim_seq == Some(s.seq))
                .map(|f| format!("{} ({})", esc(&f.finding.title), esc(&f.finding.id)))
                .collect();
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                s.seq,
                esc(source.as_str()),
                esc(text),
                if names.is_empty() {
                    "-".to_string()
                } else {
                    names
                        .iter()
                        .map(|n| esc(n))
                        .collect::<Vec<_>>()
                        .join("<br>")
                },
                if borne_on.is_empty() {
                    "none recorded".to_string()
                } else {
                    borne_on.join("<br>")
                }
            ));
        }
        body.push_str("</table>");
    }

    body.push_str("<h2>Unplanned work</h2>");
    let unplanned: Vec<_> = a
        .findings
        .iter()
        .filter(|f| f.is_open() && f.checker() == Some("baseline_diff"))
        .collect();
    if unplanned.is_empty() {
        body.push_str("<p>Nothing the plan or a prompt did not ask for was found.</p>");
    } else {
        body.push_str("<ul>");
        for f in unplanned {
            body.push_str(&format!(
                "<li>[{}] {} ({}, record {})</li>",
                esc(&format!("{:?}", f.finding.severity)),
                esc(&f.finding.title),
                esc(f.finding.tier.as_str()),
                f.seq
            ));
        }
        body.push_str("</ul>");
    }

    body.push_str(&format!(
        "<h2>The report</h2><pre>{}</pre>\
         <p>This is the report exactly as <code>vdt report</code> prints it.</p>",
        esc(&report)
    ));
    Ok(Some(page(&format!("Report for session {id}"), &body)))
}

pub(crate) async fn session_page(
    State(st): State<St>,
    Path(id): Path<String>,
    Query(q): Query<RecordedQuery>,
) -> Response {
    let (root, home, token) = (st.root.clone(), st.home.clone(), st.token.clone());
    let r = tokio::task::spawn_blocking(move || {
        session_html(&root, home.as_deref(), &token, &id, q.recorded, q.ran)
    })
    .await;
    match r {
        Ok(Ok(Some(html))) => Html(html).into_response(),
        Ok(Ok(None)) => not_found("There is no such session in this project's ledger."),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

#[derive(Deserialize)]
pub(crate) struct DecideForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    reason: String,
}

fn parse_action(f: &DecideForm) -> Result<Action, String> {
    Ok(match f.action.as_str() {
        "accept_finding" => Action::AcceptFinding(f.value.clone()),
        "accept_translation" => Action::AcceptPromptTranslation(
            f.value
                .trim()
                .parse()
                .map_err(|_| format!("{:?} is not a claim record number", f.value))?,
        ),
        "approve_commands" => Action::ApproveCommands,
        "raise_lane" => Action::RaiseLane(lane_from_text(&f.value)?),
        "set_mode" => Action::SetMode(f.value.clone()),
        "pause" => Action::Pause(f.value.clone()),
        "commit_anyway" => Action::CommitAnyway {
            reason: f.reason.clone(),
            tree: None,
        },
        "plan_file" => Action::SelectPlan(f.value.clone()),
        "withdraw_prompt" => Action::WithdrawPrompt(
            f.value
                .trim()
                .parse()
                .map_err(|_| format!("{:?} is not a prompt number", f.value))?,
        ),
        "waive_words" => Action::WaiveWords(f.value.clone()),
        "mcp_install" => Action::McpInstall(f.value.clone()),
        other => return Err(format!("{other:?} is not something this page can record")),
    })
}

pub(crate) async fn decide(State(st): State<St>, Form(form): Form<DecideForm>) -> Response {
    if form.token != st.token {
        return deny(
            StatusCode::FORBIDDEN,
            "Refused: the page token is missing or wrong. Reload the page and press the button again.",
        );
    }
    let action = match parse_action(&form) {
        Ok(a) => a,
        Err(why) => {
            return deny(
                StatusCode::BAD_REQUEST,
                &format!("Nothing was recorded. {why}"),
            );
        }
    };
    let (root, home) = (st.root.clone(), st.home.clone());
    let session = form.session.clone();
    let r = tokio::task::spawn_blocking(
        move || -> Result<Result<(u64, String), String>, LedgerError> {
            let mut l = Ledger::open(&root)?;
            let Some(id) = resolve_session(&l, Some(&session)).ok().flatten() else {
                return Ok(Err(
                    "There is no such session in this project's ledger.".into()
                ));
            };
            Ok(apply(&root, &mut l, &id, &action, home.as_deref()).map(|seq| (seq, id)))
        },
    )
    .await;
    match r {
        Ok(Ok(Ok((seq, id)))) => {
            Redirect::to(&format!("/session/{id}?recorded={seq}")).into_response()
        }
        Ok(Ok(Err(why))) => deny(
            StatusCode::BAD_REQUEST,
            &format!("Nothing was recorded. {why}"),
        ),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

// ---------------------------------------------------------------- page-run actions

#[derive(Deserialize)]
pub(crate) struct TokenForm {
    #[serde(default)]
    token: String,
}

/// `POST /session/<id>/check`: run the lane's checkers and desks now and record the result.
pub(crate) async fn run_check(
    State(st): State<St>,
    Path(id): Path<String>,
    Form(form): Form<TokenForm>,
) -> Response {
    if form.token != st.token {
        return deny(
            StatusCode::FORBIDDEN,
            "Refused: the page token is missing or wrong. Reload the page and press the button again.",
        );
    }
    let (root, home) = (st.root.clone(), st.home.clone());
    let wanted = id.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<u64>, LedgerError> {
        let mut l = Ledger::open(&root)?;
        let Some(session) = resolve_session(&l, Some(&wanted)).ok().flatten() else {
            return Ok(None);
        };
        let home = home.as_deref();
        match verdict_engine::verify(
            &root,
            &mut l,
            &session,
            home,
            &verdict_engine::VerifyOptions {
                budget: None,
                reason: Some("the page's Run the checkers button".into()),
            },
        ) {
            Ok(v) => Ok(Some(v.gate_seq)),
            Err(reason) => {
                // The run could not even start; say so in the ledger, never as a pass.
                l.append(
                    Writer::Core,
                    Some(&session),
                    Record::CheckUnavailable {
                        component: "verification".into(),
                        reason: format!("the page's run failed: {reason}"),
                    },
                )?;
                Ok(Some(0))
            }
        }
    })
    .await;
    match r {
        Ok(Ok(Some(seq))) if seq > 0 => {
            Redirect::to(&format!("/session/{id}?ran={seq}")).into_response()
        }
        Ok(Ok(Some(_))) => Redirect::to(&format!("/session/{id}")).into_response(),
        Ok(Ok(None)) => deny(
            StatusCode::BAD_REQUEST,
            "Nothing ran: there is no such session. Nothing was recorded as checked.",
        ),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

/// `POST /session/<id>/desks`: dispatch only the lane's desks.
pub(crate) async fn run_desks(
    State(st): State<St>,
    Path(id): Path<String>,
    Form(form): Form<TokenForm>,
) -> Response {
    if form.token != st.token {
        return deny(
            StatusCode::FORBIDDEN,
            "Refused: the page token is missing or wrong. Reload the page and press the button again.",
        );
    }
    let (root, home) = (st.root.clone(), st.home.clone());
    let wanted = id.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<bool, LedgerError> {
        let mut l = Ledger::open(&root)?;
        let Some(session) = resolve_session(&l, Some(&wanted)).ok().flatten() else {
            return Ok(false);
        };
        let live = live_input(&root, &l, &session, home.as_deref())?;
        let lane = verdict_core::lane::effective_lane(&l, &session, live.input.implied_lane)?;
        let runner = verdict_engine::RealRunner::with_home(home.clone());
        match verdict_core::gate::CheckRunner::run_desks(&runner, &mut l, &root, &session, lane) {
            Ok(()) => Ok(true),
            Err(reason) => {
                l.append(
                    Writer::Core,
                    Some(&session),
                    Record::CheckUnavailable {
                        component: "desks".into(),
                        reason,
                    },
                )?;
                Ok(false)
            }
        }
    })
    .await;
    match r {
        Ok(Ok(_)) => Redirect::to(&format!("/session/{id}")).into_response(),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

/// `GET /decisions.jsonl`: the project's human decisions as JSON lines.
pub(crate) async fn decisions_download(State(st): State<St>) -> Response {
    let root = st.root.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<Option<String>, LedgerError> {
        let Some(l) = Ledger::open_existing(&root)? else {
            return Ok(None);
        };
        verdict_core::export::decisions_jsonl(&l).map(Some)
    })
    .await;
    match r {
        Ok(Ok(Some(text))) => crate::download(
            text,
            "application/x-ndjson; charset=utf-8",
            "decisions.jsonl",
        ),
        Ok(Ok(None)) => not_found("There is no ledger in this project yet."),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}

#[derive(Deserialize)]
pub(crate) struct McpForm {
    #[serde(default)]
    token: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    host: String,
}

/// `POST /mcp-install`: register Verdict's MCP server with one host (a human action).
pub(crate) async fn mcp_install(State(st): State<St>, Form(form): Form<McpForm>) -> Response {
    if form.token != st.token {
        return deny(
            StatusCode::FORBIDDEN,
            "Refused: the page token is missing or wrong. Reload the page and press the button again.",
        );
    }
    let Some(host) = verdict_adapters::McpHost::parse(&form.host) else {
        return deny(StatusCode::BAD_REQUEST, "Unknown host.");
    };
    let (root, home, session) = (st.root.clone(), st.home.clone(), form.session.clone());
    let r = tokio::task::spawn_blocking(
        move || -> Result<Result<(u64, String), String>, LedgerError> {
            let req = verdict_adapters::McpRequest {
                host,
                project: Some(root.clone()),
                vdt_path: Some("vdt".into()),
                codex_home: home.as_ref().map(|h| h.join(".codex")),
                i_am_the_human: true,
            };
            let mut out = Vec::new();
            let code = verdict_adapters::mcp_command(&req, &root, &mut out)
                .map_err(LedgerError::Invalid)?;
            let text = String::from_utf8_lossy(&out).to_string();
            if code != 0 {
                return Ok(Err(text));
            }
            let mut l = Ledger::open(&root)?;
            let id = resolve_session(&l, Some(&session)).ok().flatten();
            let a = l.append(
                Writer::Human,
                id.as_deref(),
                Record::Decision {
                    decision: DecisionKind::McpInstall,
                    choice: form.host.clone(),
                    reason: None,
                    who: Writer::Human,
                },
            )?;
            Ok(Ok((a.seq, text)))
        },
    )
    .await;
    match r {
        Ok(Ok(Ok((seq, text)))) => Html(page(
            "Recorded: register the MCP server",
            &format!(
                "<div class=\"note\"><strong>Your decision is recorded</strong> as record {seq} \
                 (written by: human).</div><pre>{}</pre>\
                 <p><a href=\"/\">Back to the sessions list</a></p>",
                esc(&text)
            ),
        ))
        .into_response(),
        Ok(Ok(Err(why))) => deny(StatusCode::BAD_REQUEST, &why),
        Ok(Err(e)) => error_page(&e),
        Err(e) => error_page(&LedgerError::Invalid(e.to_string())),
    }
}
