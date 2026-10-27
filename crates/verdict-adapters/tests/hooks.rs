mod common;

use common::*;
use std::io::Read;
use std::time::{Duration, Instant};
use verdict_adapters::run_hook;
use verdict_core::{ClaimSource, Record, Writer};

#[test]
fn claude_payloads_become_session_prompt_tool_calls_and_a_final_message() {
    let p = Project::new();
    run_fixture(&p, "claude", CLAUDE);
    let recs = p.records();
    assert_eq!(
        kinds(&recs),
        ["session", "prompt", "tool_call", "tool_call", "claim"]
    );
    assert!(recs.iter().all(|s| s.writer == Writer::Adapter));
    let sid = recs[0].session_id.clone().unwrap();
    assert!(recs.iter().all(|s| s.session_id.as_deref() == Some(&sid)));

    let Record::Session {
        agent,
        model,
        vendor,
        baseline,
        ..
    } = &recs[0].record
    else {
        panic!()
    };
    assert_eq!(
        (agent.as_str(), model.as_str(), vendor.as_str()),
        ("claude", "claude-sonnet-5-5", "anthropic")
    );
    assert!(
        !baseline.hash.is_empty(),
        "the first event writes the baseline"
    );

    let Record::Prompt { ordinal, text, .. } = &recs[1].record else {
        panic!()
    };
    assert_eq!((*ordinal, text.as_str()), (1, "add a login page"));

    let Record::ToolCall {
        tool,
        target,
        output_hash,
        duration_ms,
    } = &recs[2].record
    else {
        panic!()
    };
    assert_eq!((tool.as_str(), target.as_str()), ("shell", "cargo test"));
    assert_eq!(
        output_hash,
        &blake3::hash(b"test result: ok. 3 passed\n")
            .to_hex()
            .to_string(),
        "the hash is of the output text, which is never stored"
    );
    assert_eq!(*duration_ms, 1200);

    let Record::ToolCall {
        tool,
        target,
        output_hash,
        ..
    } = &recs[3].record
    else {
        panic!()
    };
    assert_eq!(
        (tool.as_str(), target.as_str(), output_hash.as_str()),
        ("edit", "src/login.rs", ""),
        "an edit target is project-relative; only shell output is hashed"
    );

    let Record::Claim {
        source,
        text,
        prompt_ordinal,
        ..
    } = &recs[4].record
    else {
        panic!()
    };
    assert_eq!(*source, ClaimSource::FinalMessage);
    assert_eq!(text, "Login page added; tests pass.");
    assert_eq!(*prompt_ordinal, Some(1));
}

#[test]
fn codex_payloads_become_the_same_kinds_of_records() {
    let p = Project::new();
    run_fixture(&p, "codex", CODEX);
    let recs = p.records();
    assert_eq!(
        kinds(&recs),
        ["session", "prompt", "tool_call", "tool_call", "claim"]
    );
    let Record::Session { agent, vendor, .. } = &recs[0].record else {
        panic!()
    };
    assert_eq!((agent.as_str(), vendor.as_str()), ("codex", "openai"));
    let Record::ToolCall { tool, target, .. } = &recs[3].record else {
        panic!()
    };
    assert_eq!(
        (tool.as_str(), target.as_str()),
        ("edit", "src/login.rs"),
        "the apply_patch body names the file"
    );
    // Codex sends no duration: it is measured from Pre to Post, so it is small but present.
    let Record::ToolCall { duration_ms, .. } = &recs[2].record else {
        panic!()
    };
    assert!(*duration_ms < 10_000);
}

fn normalized(p: &Project) -> Vec<(String, String, serde_json::Value)> {
    let recs = p.records();
    let sid = recs[0].session_id.clone().unwrap();
    recs.iter()
        .map(|s| {
            assert_eq!(s.session_id.as_deref(), Some(sid.as_str()));
            let mut v = serde_json::to_value(&s.record).unwrap();
            let o = v.as_object_mut().unwrap();
            for k in [
                "id",
                "started_at",
                "baseline",
                "agent",
                "model",
                "vendor",
                "policy_hash",
                "duration_ms",
            ] {
                o.remove(k);
            }
            (s.writer.to_string(), s.kind.to_string(), v)
        })
        .collect()
}

#[test]
fn parity_same_logical_session_gives_the_same_records_on_all_four_hosts() {
    let a = Project::new();
    let b = Project::new();
    let c = Project::new();
    let d = Project::new();
    run_fixture(&a, "claude", CLAUDE);
    run_fixture(&b, "codex", CODEX);
    run_fixture(&c, "cursor", CURSOR);
    run_fixture(&d, "antigravity", ANTIGRAVITY);
    let (na, nb, nc, nd) = (
        normalized(&a),
        normalized(&b),
        normalized(&c),
        normalized(&d),
    );
    assert_eq!(na.len(), 5);
    assert_eq!(
        na, nb,
        "records differ beyond host name, raw ids, model and timings"
    );
    assert_eq!(nc, na, "Cursor gives the same ledger records");
    assert_eq!(nd, na, "Antigravity gives the same ledger records");
}

#[test]
fn cursor_and_antigravity_record_their_own_host_name_and_vendor() {
    let c = Project::new();
    run_fixture(&c, "cursor", CURSOR);
    let recs = c.records();
    let Record::Session {
        agent,
        model,
        vendor,
        ..
    } = &recs[0].record
    else {
        panic!()
    };
    assert_eq!(
        (agent.as_str(), model.as_str(), vendor.as_str()),
        ("cursor", "claude-sonnet-5-5", "anthropic"),
        "a Cursor session's vendor comes from the model it served"
    );

    let a = Project::new();
    run_fixture(&a, "antigravity", ANTIGRAVITY);
    let recs = a.records();
    let Record::Session {
        agent,
        model,
        vendor,
        ..
    } = &recs[0].record
    else {
        panic!()
    };
    assert_eq!(
        (agent.as_str(), model.as_str(), vendor.as_str()),
        ("antigravity", "gemini-3-pro", "google")
    );
    let Record::Prompt { text, .. } = &recs[1].record else {
        panic!()
    };
    assert_eq!(text, "add a login page", "PreInvocation carries the prompt");
    let Record::ToolCall { tool, target, .. } = &recs[2].record else {
        panic!()
    };
    assert_eq!((tool.as_str(), target.as_str()), ("shell", "cargo test"));
}

#[test]
fn one_ledger_session_per_host_session_and_any_event_can_open_it() {
    let p = Project::new();
    // The hook was installed mid-session: the first event seen is a tool call, not SessionStart.
    let post = |sid: &str, cmd: &str| {
        format!(
            r#"{{"session_id":"{sid}","cwd":{},"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"{cmd}"}},"tool_response":{{"stdout":"x","stderr":""}}}}"#,
            serde_json::to_string(&p.dir.path().to_string_lossy().to_string()).unwrap()
        )
    };
    run_bytes(&p, "claude", "PostToolUse", post("A", "one").as_bytes());
    run_bytes(&p, "claude", "PostToolUse", post("A", "two").as_bytes());
    run_bytes(&p, "claude", "PostToolUse", post("B", "three").as_bytes());
    run_bytes(&p, "codex", "PostToolUse", post("A", "four").as_bytes());
    let recs = p.records();
    assert_eq!(
        recs.iter().filter(|s| s.kind.as_str() == "session").count(),
        3
    );
    let sid_of = |cmd: &str| {
        recs.iter()
            .find(|s| matches!(&s.record, Record::ToolCall { target, .. } if target == cmd))
            .and_then(|s| s.session_id.clone())
            .unwrap()
    };
    assert_eq!(sid_of("one"), sid_of("two"));
    assert_ne!(sid_of("one"), sid_of("three"));
    assert_ne!(
        sid_of("one"),
        sid_of("four"),
        "same raw id on another host is another session"
    );
}

#[test]
fn repeated_session_start_does_not_open_a_second_session() {
    let p = Project::new();
    let steps = playback(CLAUDE, p.dir.path());
    for _ in 0..3 {
        run_bytes(&p, "claude", &steps[0].0, steps[0].1.as_bytes());
    }
    assert_eq!(kinds(&p.records()), ["session"]);
}

#[test]
fn codex_turn_id_ties_the_final_message_to_its_prompt() {
    let p = Project::new();
    let cwd = serde_json::to_string(&p.dir.path().to_string_lossy().to_string()).unwrap();
    let ev = |name: &str, turn: &str, extra: &str| {
        format!(
            r#"{{"session_id":"S","turn_id":"{turn}","cwd":{cwd},"hook_event_name":"{name}"{extra}}}"#
        )
    };
    run_bytes(
        &p,
        "codex",
        "UserPromptSubmit",
        ev("UserPromptSubmit", "t1", r#","prompt":"first""#).as_bytes(),
    );
    run_bytes(
        &p,
        "codex",
        "UserPromptSubmit",
        ev("UserPromptSubmit", "t2", r#","prompt":"second""#).as_bytes(),
    );
    // A late Stop for the first turn still belongs to the first prompt.
    run_bytes(
        &p,
        "codex",
        "Stop",
        ev("Stop", "t1", r#","last_assistant_message":"done one""#).as_bytes(),
    );
    run_bytes(
        &p,
        "codex",
        "Stop",
        ev("Stop", "t2", r#","last_assistant_message":"done two""#).as_bytes(),
    );
    let claims: Vec<_> = p
        .records()
        .into_iter()
        .filter_map(|s| match s.record {
            Record::Claim {
                text,
                prompt_ordinal,
                ..
            } => Some((text, prompt_ordinal)),
            _ => None,
        })
        .collect();
    assert_eq!(
        claims,
        [
            ("done one".to_string(), Some(1)),
            ("done two".to_string(), Some(2))
        ]
    );
}

#[test]
fn prompts_count_up_within_a_session() {
    let p = Project::new();
    let cwd = serde_json::to_string(&p.dir.path().to_string_lossy().to_string()).unwrap();
    for text in ["a", "b", "c"] {
        let body = format!(
            r#"{{"session_id":"S","cwd":{cwd},"hook_event_name":"UserPromptSubmit","prompt":"{text}"}}"#
        );
        run_bytes(&p, "claude", "UserPromptSubmit", body.as_bytes());
    }
    let ordinals: Vec<u32> = p
        .records()
        .iter()
        .filter_map(|s| match &s.record {
            Record::Prompt { ordinal, .. } => Some(*ordinal),
            _ => None,
        })
        .collect();
    assert_eq!(ordinals, [1, 2, 3]);
}

#[test]
fn event_names_may_be_spelled_several_ways() {
    let p = Project::new();
    let steps = playback(CLAUDE, p.dir.path());
    run_bytes(&p, "Claude", "session-start", steps[0].1.as_bytes());
    run_bytes(&p, "claude", "user_prompt_submit", steps[1].1.as_bytes());
    assert_eq!(kinds(&p.records()), ["session", "prompt"]);
}

// ---- fail open ------------------------------------------------------------------------------

fn only_unavailable(p: &Project) -> Vec<String> {
    let recs = p.records();
    assert!(
        recs.iter().all(|s| s.kind.as_str() == "check_unavailable"),
        "{:?}",
        kinds(&recs)
    );
    assert!(recs.iter().all(|s| s.writer == Writer::Adapter));
    unavailable_reasons(&recs)
}

#[test]
fn bad_json_is_survived_and_recorded_as_check_unavailable() {
    let p = Project::new();
    let _g = enter(&p);
    run_bytes(&p, "claude", "PostToolUse", b"{ this is not json");
    run_bytes(&p, "codex", "Stop", b"");
    run_bytes(&p, "claude", "Stop", b"[1,2,3]");
    let reasons = only_unavailable(&p);
    assert_eq!(reasons.len(), 3);
    assert!(reasons[0].contains("not JSON"), "{reasons:?}");
    assert!(reasons[2].contains("not an object"), "{reasons:?}");
}

#[test]
fn unknown_event_unknown_host_and_missing_session_id_are_recorded_not_fatal() {
    let p = Project::new();
    let cwd = serde_json::to_string(&p.dir.path().to_string_lossy().to_string()).unwrap();
    let body = format!(r#"{{"session_id":"S","cwd":{cwd}}}"#);
    run_bytes(&p, "claude", "SessionEnd", body.as_bytes());
    run_bytes(&p, "gemini", "Stop", body.as_bytes());
    run_bytes(
        &p,
        "claude",
        "Stop",
        format!(r#"{{"cwd":{cwd}}}"#).as_bytes(),
    );
    let reasons = only_unavailable(&p);
    assert!(reasons[0].contains("unknown event"), "{reasons:?}");
    assert!(reasons[1].contains("unknown host"), "{reasons:?}");
    assert!(reasons[2].contains("no session_id"), "{reasons:?}");
}

#[test]
fn a_stop_without_the_final_message_says_so_inside_the_session() {
    let p = Project::new();
    let steps = playback(CLAUDE, p.dir.path());
    run_bytes(&p, "claude", "SessionStart", steps[0].1.as_bytes());
    let cwd = serde_json::to_string(&p.dir.path().to_string_lossy().to_string()).unwrap();
    let stop =
        format!(r#"{{"session_id":"claude-sess-0001","cwd":{cwd},"hook_event_name":"Stop"}}"#);
    run_bytes(&p, "claude", "Stop", stop.as_bytes());
    let recs = p.records();
    assert_eq!(kinds(&recs), ["session", "check_unavailable"]);
    assert_eq!(
        recs[0].session_id, recs[1].session_id,
        "tied to the session it concerns"
    );
}

#[test]
fn a_locked_ledger_does_not_stop_the_agent_and_returns_inside_the_budget() {
    let p = Project::new();
    // Create the ledger and session first, then lock the database for writes.
    let steps = playback(CLAUDE, p.dir.path());
    run_bytes(&p, "claude", "SessionStart", steps[0].1.as_bytes());
    let conn = rusqlite::Connection::open(verdict_core::ledger_path(p.dir.path())).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let t = Instant::now();
    run_hook(
        "claude",
        "UserPromptSubmit",
        Box::new(std::io::Cursor::new(steps[1].1.clone().into_bytes())),
        p.opts(),
        Duration::from_millis(400),
    );
    let took = t.elapsed();
    assert!(took < Duration::from_secs(4), "took {took:?}");
    conn.execute_batch("ROLLBACK").unwrap();
    drop(conn);
    assert!(
        !kinds(&p.records()).contains(&"prompt"),
        "the prompt could not be written while the database was locked"
    );
}

struct Never;
impl Read for Never {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_secs(3600));
        Ok(0)
    }
}

#[test]
fn a_stdin_that_never_closes_is_abandoned_at_the_budget_and_recorded() {
    let p = Project::new();
    let _g = enter(&p);
    let t = Instant::now();
    run_hook(
        "codex",
        "Stop",
        Box::new(Never),
        p.opts(),
        Duration::from_millis(300),
    );
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "took {:?}",
        t.elapsed()
    );
    let reasons = only_unavailable(&p);
    assert!(reasons[0].contains("did not finish"), "{reasons:?}");
}

#[test]
fn the_stop_budget_plus_the_failure_record_fits_inside_twenty_seconds() {
    use verdict_adapters::hook::{OTHER_BUDGET, STOP_BUDGET, UNAVAILABLE_BUDGET};
    assert!(STOP_BUDGET + UNAVAILABLE_BUDGET < Duration::from_secs(20));
    assert!(OTHER_BUDGET < STOP_BUDGET);
}

#[test]
fn secrets_in_prompts_commands_and_final_messages_never_reach_the_ledger() {
    let p = Project::new();
    let cwd = p.dir.path().to_string_lossy().to_string();
    let send = |event: &str, extra: serde_json::Value| {
        let mut v = serde_json::json!({
            "session_id": "claude-sess-secret",
            "cwd": cwd,
            "hook_event_name": event,
            "model": "m",
        });
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        run_bytes(&p, "claude", event, v.to_string().as_bytes());
    };
    send(
        "UserPromptSubmit",
        serde_json::json!({ "prompt": "use API_KEY=promptsecret111 for the call" }),
    );
    send(
        "PostToolUse",
        serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": "curl -H \"Authorization: Bearer cmdsecret222\" https://x.test" },
            "tool_response": { "stdout": "ok" }
        }),
    );
    send(
        "Stop",
        serde_json::json!({ "last_assistant_message": "Done. Your token: ghp_abcdefghijklmnopqrstuvwxyz0123" }),
    );
    let dump = serde_json::to_string(
        &p.records()
            .iter()
            .map(|s| serde_json::to_value(&s.record).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for secret in [
        "promptsecret111",
        "cmdsecret222",
        "ghp_abcdefghijklmnopqrstuvwxyz0123",
    ] {
        assert!(
            !dump.contains(secret),
            "{secret} reached the ledger: {dump}"
        );
    }
    assert_eq!(
        kinds(&p.records()),
        ["session", "prompt", "tool_call", "claim"]
    );
}
