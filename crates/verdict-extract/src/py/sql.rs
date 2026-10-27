//! A small, forgiving reader for SQL text: which statement it is and which table it touches.
//! It reads words, not a grammar, and says `None` rather than guess.

use crate::model::WriteOp;

/// A data-changing statement found in SQL text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlWrite {
    /// What it does.
    pub op: WriteOp,
    /// The table, when it is a plain name.
    pub table: Option<String>,
}

fn tokens(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let cs: Vec<char> = sql.chars().collect();
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(std::mem::take(cur));
        }
    };
    while i < cs.len() {
        let c = cs[i];
        match c {
            '-' if cs.get(i + 1) == Some(&'-') => {
                flush(&mut cur, &mut out);
                while i < cs.len() && cs[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if cs.get(i + 1) == Some(&'*') => {
                flush(&mut cur, &mut out);
                i += 2;
                while i + 1 < cs.len() && !(cs[i] == '*' && cs[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            '\'' => {
                flush(&mut cur, &mut out);
                i += 1;
                while i < cs.len() {
                    if cs[i] == '\'' {
                        if cs.get(i + 1) == Some(&'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                out.push("'str'".into());
            }
            '"' | '`' => {
                let q = c;
                i += 1;
                while i < cs.len() && cs[i] != q {
                    cur.push(cs[i]);
                    i += 1;
                }
            }
            '(' | ')' | ',' | ';' => {
                flush(&mut cur, &mut out);
                out.push(c.to_string());
            }
            c if c.is_whitespace() => flush(&mut cur, &mut out),
            c => cur.push(c),
        }
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

const NOT_A_TABLE: &[&str] = &[
    "SET", "SELECT", "FROM", "WHERE", "VALUES", "CASCADE", "RESTRICT", "NOWAIT", "OF", "SKIP",
    "NO", "DEFAULT", "NULL", "ONLY", "IF", "TABLE", "INTO", "USING", "AS", "WITH", "(", ")", ",",
    ";", "'str'",
];

fn table_name(tok: Option<&String>) -> Option<String> {
    let t = tok?;
    let up = t.to_ascii_uppercase();
    if NOT_A_TABLE.contains(&up.as_str()) {
        return None;
    }
    let first = t.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    if !t
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return None;
    }
    Some(t.clone())
}

/// The first token at index `i` or after it that is not one of `skip` (case-insensitive).
fn after<'a>(toks: &'a [String], mut i: usize, skip: &[&str]) -> Option<&'a String> {
    while let Some(t) = toks.get(i) {
        if skip.iter().any(|s| t.eq_ignore_ascii_case(s)) {
            i += 1;
        } else {
            return Some(t);
        }
    }
    None
}

/// The write statement at the start of `sql` (after any `WITH` clause), if it is one.
pub fn write_of(sql: &str) -> Option<SqlWrite> {
    let toks = tokens(sql);
    let mut depth = 0i32;
    let mut start = None;
    for (i, t) in toks.iter().enumerate() {
        match t.as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ if depth == 0 => {
                let up = t.to_ascii_uppercase();
                if i == 0 && up == "WITH" {
                    continue;
                }
                if i == 0 {
                    start = Some(i);
                    break;
                }
                if matches!(
                    up.as_str(),
                    "INSERT" | "UPDATE" | "DELETE" | "SELECT" | "MERGE"
                ) {
                    start = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let i = start?;
    let kw = toks[i].to_ascii_uppercase();
    let rest = &toks[i + 1..];
    let has_conflict = || {
        rest.windows(2).any(|w| {
            w[0].eq_ignore_ascii_case("ON")
                && (w[1].eq_ignore_ascii_case("CONFLICT") || w[1].eq_ignore_ascii_case("DUPLICATE"))
        })
    };
    match kw.as_str() {
        "INSERT" => {
            let mut op = WriteOp::Insert;
            let mut j = 0;
            if rest.first().is_some_and(|t| t.eq_ignore_ascii_case("OR")) {
                if rest
                    .get(1)
                    .is_some_and(|t| t.eq_ignore_ascii_case("REPLACE"))
                {
                    op = WriteOp::Upsert;
                }
                j = 2;
            }
            if has_conflict() {
                op = WriteOp::Upsert;
            }
            let into = rest.get(j)?;
            if !into.eq_ignore_ascii_case("INTO") {
                return None;
            }
            Some(SqlWrite {
                op,
                table: table_name(after(rest, j + 1, &["ONLY"])),
            })
        }
        "REPLACE" | "UPSERT" | "MERGE" => {
            let into = rest.first()?;
            if !into.eq_ignore_ascii_case("INTO") {
                return None;
            }
            Some(SqlWrite {
                op: WriteOp::Upsert,
                table: table_name(after(rest, 1, &["ONLY"])),
            })
        }
        "UPDATE" => Some(SqlWrite {
            op: WriteOp::Update,
            table: table_name(after(rest, 0, &["ONLY"])),
        }),
        "DELETE" => {
            if !rest.first().is_some_and(|t| t.eq_ignore_ascii_case("FROM")) {
                return None;
            }
            Some(SqlWrite {
                op: WriteOp::Delete,
                table: table_name(after(rest, 1, &["ONLY"])),
            })
        }
        "TRUNCATE" => Some(SqlWrite {
            op: WriteOp::Delete,
            table: table_name(after(rest, 0, &["TABLE", "ONLY"])),
        }),
        _ => None,
    }
}

/// Every table a script creates, alters, drops or changes the rows of, in order, without repeats.
pub fn tables_of(sql: &str) -> Vec<String> {
    let toks = tokens(sql);
    let mut out: Vec<String> = Vec::new();
    let mut push = |t: Option<String>| {
        if let Some(t) = t
            && !out.contains(&t)
        {
            out.push(t);
        }
    };
    for i in 0..toks.len() {
        let up = toks[i].to_ascii_uppercase();
        let prev = i
            .checked_sub(1)
            .map(|p| toks[p].to_ascii_uppercase())
            .unwrap_or_default();
        let next = |k: usize| toks.get(i + k).map(|t| t.to_ascii_uppercase());
        match up.as_str() {
            "INSERT" | "REPLACE" | "UPSERT" | "MERGE" => {
                // INSERT [OR x] INTO t
                let mut j = i + 1;
                if toks.get(j).is_some_and(|t| t.eq_ignore_ascii_case("OR")) {
                    j += 2;
                }
                if toks.get(j).is_some_and(|t| t.eq_ignore_ascii_case("INTO")) {
                    push(table_name(after(&toks, j + 1, &["ONLY"])));
                }
            }
            "UPDATE" if !matches!(prev.as_str(), "DO" | "FOR" | "ON") => {
                push(table_name(after(&toks, i + 1, &["ONLY"])));
            }
            "DELETE" if next(1).as_deref() == Some("FROM") => {
                push(table_name(after(&toks, i + 2, &["ONLY"])));
            }
            "TRUNCATE" => push(table_name(after(&toks, i + 1, &["TABLE", "ONLY"]))),
            "TABLE"
                if matches!(
                    prev.as_str(),
                    "CREATE" | "ALTER" | "DROP" | "TEMP" | "TEMPORARY" | "UNLOGGED"
                ) =>
            {
                push(table_name(after(
                    &toks,
                    i + 1,
                    &["IF", "NOT", "EXISTS", "ONLY"],
                )));
            }
            _ => {}
        }
    }
    out
}

/// True when the SQL has an asyncpg-style `$1` placeholder.
pub fn has_dollar_param(sql: &str) -> bool {
    let b = sql.as_bytes();
    b.windows(2).any(|w| w[0] == b'$' && w[1].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements() {
        let w = |s: &str| write_of(s).map(|w| (w.op, w.table));
        assert_eq!(
            w("INSERT INTO users (a) VALUES ($1)"),
            Some((WriteOp::Insert, Some("users".into())))
        );
        assert_eq!(
            w("  insert into public.\"Notes\"(a) values (1) on conflict (a) do update set b=1"),
            Some((WriteOp::Upsert, Some("public.Notes".into())))
        );
        assert_eq!(
            w("UPDATE items SET a = 1"),
            Some((WriteOp::Update, Some("items".into())))
        );
        assert_eq!(
            w("DELETE FROM ONLY items WHERE id=1"),
            Some((WriteOp::Delete, Some("items".into())))
        );
        assert_eq!(
            w("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x"),
            Some((WriteOp::Insert, Some("t".into())))
        );
        assert_eq!(w("SELECT * FROM t FOR UPDATE"), None);
        assert_eq!(w("CREATE TABLE t (id int)"), None);
        assert_eq!(
            w("INSERT INTO ${} (a) VALUES (1)"),
            Some((WriteOp::Insert, None))
        );
        assert_eq!(
            w("-- hi\nTRUNCATE TABLE logs").map(|w| w.0),
            Some(WriteOp::Delete)
        );
    }

    #[test]
    fn migration_tables() {
        assert_eq!(
            tables_of(
                "CREATE TABLE IF NOT EXISTS a (x int); ALTER TABLE b ADD y int; INSERT INTO c SELECT 1; DROP TABLE a"
            ),
            ["a", "b", "c"]
        );
        assert!(tables_of("SELECT 1 FOR UPDATE").is_empty());
    }
}
