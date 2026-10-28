//! A stand-in for the `codex` CLI in the adapter's tests only. It accepts `exec ... -o <path> -`,
//! reads stdin, writes a desk answer to `-o`, and prints one JSONL event with an observed model and
//! usage. Nothing in the product calls it.

use std::io::{Read, Write};

fn main() {
    let mut out = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == "-o" {
            out = it.next();
        }
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(out) = out else {
        eprintln!("the stub was not given -o");
        std::process::exit(2);
    };
    if let Err(e) = std::fs::write(&out, r#"{"findings":[],"hypotheses":[]}"#) {
        eprintln!("cannot write {out}: {e}");
        std::process::exit(2);
    }
    let mut stdout = std::io::stdout();
    let _ = writeln!(
        stdout,
        r#"{{"type":"turn.completed","model":"gpt-observed","usage":{{"input_tokens":3,"output_tokens":2}}}}"#
    );
}
