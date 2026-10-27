//! Blank out secrets before captured text goes into the ledger.
//!
//! The ledger is append-only, so a secret written there can never be taken out again. Prompts,
//! the agent's final message and shell commands are passed through [`redact`] first. It replaces
//! the secret part with `[REDACTED]` and keeps the rest, so the record still says what happened.
//!
//! What counts as a secret:
//! * a private key block (`-----BEGIN ... PRIVATE KEY-----` to its `END` line);
//! * the word after `Bearer` or `Basic` (an `Authorization` header value);
//! * the value in `name=value` or `name: value` when the name says password, secret, token, key,
//!   auth or credential (`--password=x`, `API_KEY=x`, `"token": "x"`);
//! * the word after a flag with such a name (`--password x`, `-H "X-Api-Key: x"`);
//! * the password in a URL (`https://user:pass@host`);
//! * words with a well-known secret prefix (`sk-`, `ghp_`, `github_pat_`, `xoxb-`, `AKIA`, ...).
//!
//! This is a safety net, not a guarantee: a secret with no recognizable shape is still stored.

/// The text put in place of a secret.
pub const MARK: &str = "[REDACTED]";

const SENSITIVE_NAMES: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "access_key",
    "access-key",
    "private_key",
    "private-key",
    "auth",
    "credential",
    "session_key",
    "cookie",
];

const SECRET_PREFIXES: &[(&str, usize)] = &[
    ("sk-", 16),
    ("sk_live_", 12),
    ("sk_test_", 12),
    ("rk_live_", 12),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghs_", 20),
    ("ghu_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("glpat-", 16),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxa-", 10),
    ("xoxs-", 10),
    ("AKIA", 16),
    ("ASIA", 16),
    ("AIza", 30),
    ("npm_", 30),
    ("hf_", 30),
];

fn is_sensitive_name(name: &str) -> bool {
    let n = name
        .trim_start_matches('-')
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '{' | ',' | '$'))
        .to_ascii_lowercase();
    !n.is_empty() && SENSITIVE_NAMES.iter().any(|s| n.contains(s))
}

fn has_secret_prefix(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '_'));
    SECRET_PREFIXES.iter().any(|(p, min_rest)| {
        w.strip_prefix(p).is_some_and(|rest| {
            rest.len() >= *min_rest
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
    })
}

/// Keep the quotes and closing punctuation around a value, blank the value itself.
fn blank_value(value: &str) -> String {
    let lead: String = value
        .chars()
        .take_while(|c| matches!(c, '"' | '\''))
        .collect();
    let trail: String = {
        let mut t: Vec<char> = value
            .chars()
            .rev()
            .take_while(|c| matches!(c, '"' | '\'' | ',' | ';' | '}' | ')'))
            .collect();
        t.reverse();
        t.into_iter().collect()
    };
    if lead.len() + trail.len() >= value.len() {
        return value.to_string(); // nothing between the quotes
    }
    format!("{lead}{MARK}{trail}")
}

/// Blank the password part of `scheme://user:pass@host`.
fn redact_url_password(word: &str) -> Option<String> {
    let scheme_end = word.find("://")?;
    let rest = &word[scheme_end + 3..];
    let at = rest.find('@')?;
    let userinfo = &rest[..at];
    if userinfo.contains('/') {
        return None;
    }
    let colon = userinfo.find(':')?;
    if colon + 1 >= userinfo.len() {
        return None;
    }
    Some(format!(
        "{}{}:{MARK}{}",
        &word[..scheme_end + 3],
        &userinfo[..colon],
        &rest[at..]
    ))
}

fn redact_private_keys(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("-----BEGIN ") {
        let header_end = rest[start..]
            .find("-----\n")
            .or(rest[start + 11..].find("-----").map(|i| i + 11));
        let is_private = header_end
            .map(|e| rest[start..start + e].contains("PRIVATE KEY"))
            .unwrap_or(false);
        if !is_private {
            out.push_str(&rest[..start + 11]);
            rest = &rest[start + 11..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str("[REDACTED PRIVATE KEY]");
        match rest[start..].find("-----END ") {
            Some(e) => {
                let after_end = &rest[start + e + 9..];
                let close = after_end
                    .find("-----")
                    .map(|i| i + 5)
                    .unwrap_or(after_end.len());
                rest = &after_end[close..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Return `text` with every recognizable secret replaced by [`MARK`].
pub fn redact(text: &str) -> String {
    let text = redact_private_keys(text);
    // Split into words and the whitespace between them, so the layout is kept.
    let mut pieces: Vec<(String, bool)> = Vec::new(); // (text, is_word)
    let mut cur = String::new();
    let mut cur_is_word = None;
    for c in text.chars() {
        let w = !c.is_whitespace();
        if cur_is_word.is_some_and(|x| x != w) {
            pieces.push((std::mem::take(&mut cur), cur_is_word.unwrap_or(true)));
        }
        cur_is_word = Some(w);
        cur.push(c);
    }
    if !cur.is_empty() {
        pieces.push((cur, cur_is_word.unwrap_or(true)));
    }

    let mut blank_next = false;
    for (piece, is_word) in pieces.iter_mut() {
        if !*is_word {
            continue;
        }
        let word = piece.clone();
        let scheme_word = matches!(
            word.trim_matches(|c: char| matches!(c, '"' | '\'' | '`'))
                .to_ascii_lowercase()
                .as_str(),
            "bearer" | "basic" | "token"
        );
        if blank_next && !scheme_word {
            blank_next = false;
            *piece = blank_value(&word);
            continue;
        }
        let bare = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '`'));
        let lower = bare.to_ascii_lowercase();
        if lower == "bearer" || lower == "basic" || lower == "token" {
            blank_next = true;
            continue;
        }
        if let Some(u) = redact_url_password(&word) {
            *piece = u;
            continue;
        }
        if has_secret_prefix(&word) {
            *piece = blank_value(&word);
            continue;
        }
        // name=value or name:value inside one word.
        if let Some(pos) = word.find(['=', ':']) {
            let (name, value) = (&word[..pos], &word[pos + 1..]);
            if is_sensitive_name(name) {
                if value
                    .trim_matches(|c: char| matches!(c, '"' | '\''))
                    .is_empty()
                {
                    blank_next = true; // `"token": "x"` or `Api-Key: x`
                } else if !value.starts_with("//") {
                    *piece = format!("{name}{}{}", &word[pos..=pos], blank_value(value));
                }
                continue;
            }
        }
        // A flag or header name with a sensitive name: blank the next word.
        if (bare.starts_with('-') || bare.ends_with(':')) && is_sensitive_name(bare) {
            blank_next = true;
        }
    }
    pieces.into_iter().map(|(p, _)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hidden(input: &str, secret: &str) {
        let out = redact(input);
        assert!(!out.contains(secret), "{input:?} -> {out:?}");
        assert!(out.contains(MARK) || out.contains("[REDACTED PRIVATE KEY]"));
    }

    #[test]
    fn common_secret_shapes_are_blanked() {
        hidden(
            r#"curl -H "Authorization: Bearer abcDEF123456789xyz" https://api.example.com"#,
            "abcDEF123456789xyz",
        );
        hidden("export API_KEY=supersecretvalue1", "supersecretvalue1");
        hidden("mysql --password=hunter22 -u root", "hunter22");
        hidden("mysql --password hunter22 -u root", "hunter22");
        hidden(r#"{"token": "tok_9f8e7d6c5b4a"}"#, "tok_9f8e7d6c5b4a");
        hidden(
            "git clone https://scott:pa55word@github.com/x/y.git",
            "pa55word",
        );
        hidden(
            "use ghp_abcdefghijklmnopqrstuvwxyz0123 please",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
        );
        hidden(
            "my key is sk-proj-abcdefghijklmnop1234",
            "sk-proj-abcdefghijklmnop1234",
        );
        hidden("aws AKIAABCDEFGHIJKLMNOP done", "AKIAABCDEFGHIJKLMNOP");
        hidden(
            "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAAB3NzaC1yc2E\n-----END OPENSSH PRIVATE KEY-----\nok",
            "AAAAB3NzaC1yc2E",
        );
        hidden(r#"curl -H "X-Api-Key: k3y-v4lue" x"#, "k3y-v4lue");
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        for s in [
            "fix the login page in src/login.rs",
            "cargo test --workspace --locked",
            "open https://example.com/docs/page?x=1",
            "the tokenizer splits words",
            "rename sk-1 to sk-2",
            "",
            "  two  spaces kept  ",
        ] {
            assert_eq!(redact(s), s);
        }
    }
}
