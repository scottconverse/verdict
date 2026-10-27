mod common;

use common::*;
use proptest::prelude::*;
use serde_json::{Value, json};
use std::io::Cursor;
use std::time::{Duration, Instant};
use verdict_adapters::run_hook;

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".{0,30}".prop_map(Value::from),
    ]
}

fn any_json() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(("[a-z_]{1,12}", inner), 0..4)
                .prop_map(|kv| Value::Object(kv.into_iter().collect())),
        ]
    })
}

const FIELDS: [&str; 12] = [
    "session_id",
    "turn_id",
    "prompt_id",
    "cwd",
    "model",
    "prompt",
    "tool_name",
    "tool_input",
    "tool_response",
    "tool_use_id",
    "duration_ms",
    "last_assistant_message",
];

/// An object that uses the real field names with random values of random types.
fn payloadish() -> impl Strategy<Value = Value> {
    prop::collection::vec((prop::sample::select(FIELDS.to_vec()), any_json()), 0..10).prop_map(
        |kv| {
            let mut o = serde_json::Map::new();
            for (k, v) in kv {
                o.insert(k.to_string(), v);
            }
            // Keep cwd inside the test project most of the time by leaving it as generated;
            // the hook must cope with nonsense folders too.
            Value::Object(o)
        },
    )
}

fn hosts() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec!["claude", "codex", "other", ""])
}
fn events() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "SessionEnd",
        "",
    ])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, failure_persistence: None, .. ProptestConfig::default() })]

    /// No stdin content makes the hook hang or panic out of the call. (The process-level twin in
    /// the vdt crate checks the exit code and silence.)
    #[test]
    fn arbitrary_bytes_never_hang_or_panic(
        bytes in prop::collection::vec(any::<u8>(), 0..400),
        host in hosts(),
        event in events(),
    ) {
        let p = Project::new();
        let _g = enter(&p);
        let t = Instant::now();
        run_hook(host, event, Box::new(Cursor::new(bytes)), p.opts(), Duration::from_secs(20));
        prop_assert!(t.elapsed() < Duration::from_secs(10));
    }

    /// JSON that looks like a payload but has the wrong types in the wrong places.
    #[test]
    fn payloadish_json_never_hangs_or_panics(
        v in payloadish(),
        host in hosts(),
        event in events(),
    ) {
        let p = Project::new();
        // Point cwd at the project so any ledger the hook creates stays in the temp folder.
        let mut v = v;
        v.as_object_mut().unwrap().insert("cwd".into(), json!(p.dir.path().to_string_lossy()));
        let t = Instant::now();
        run_hook(host, event, Box::new(Cursor::new(serde_json::to_vec(&v).unwrap())), p.opts(), Duration::from_secs(20));
        prop_assert!(t.elapsed() < Duration::from_secs(10));
        // Whatever happened, the ledger (if there is one) is still a valid chain.
        if let Some(l) = verdict_core::Ledger::open_existing(p.dir.path()).unwrap() {
            prop_assert!(l.verify().unwrap().is_ok());
        }
    }
}
