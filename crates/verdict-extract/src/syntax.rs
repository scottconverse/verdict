//! Parsing and small tree helpers every extractor shares.

use crate::model::Lang;
use tree_sitter::{Language, Node, Parser, Tree};

/// The language of a path by its extension, if an extractor parses it.
pub fn lang_of(path: &str) -> Option<Lang> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "py" | "pyi" => Lang::Python,
        "ts" | "mts" | "cts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "js" | "mjs" | "cjs" => Lang::JavaScript,
        "jsx" => Lang::Jsx,
        "rs" => Lang::Rust,
        _ => return None,
    })
}

/// True for a source file in a language no extractor parses (its stack reads unverified).
pub fn is_unsupported_source(path: &str) -> bool {
    let Some((_, ext)) = path.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "go" | "java"
            | "kt"
            | "kts"
            | "scala"
            | "rb"
            | "php"
            | "cs"
            | "fs"
            | "swift"
            | "m"
            | "mm"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "vue"
            | "svelte"
            | "astro"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "clj"
            | "lua"
            | "pl"
            | "r"
            | "jl"
            | "zig"
            | "nim"
            | "elm"
            | "hs"
            | "ml"
    )
}

/// The tree-sitter grammar for a language.
pub fn grammar(lang: Lang) -> Language {
    match lang {
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::JavaScript | Lang::Jsx => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
    }
}

/// Parse `src`. `None` only when the grammar could not be loaded or parsing was cancelled.
pub fn parse(lang: Lang, src: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&grammar(lang)).ok()?;
    p.parse(src, None)
}

/// 1-based line of a node's start.
pub fn line(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

/// 1-based line of a node's end.
pub fn end_line(n: Node) -> u32 {
    n.end_position().row as u32 + 1
}

/// The source text of a node (empty when the range is not valid UTF-8).
pub fn text<'a>(n: Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

/// The value of a string literal node in any supported language, without quotes or prefixes.
///
/// Template literals and f-strings keep their literal parts and turn each interpolation into
/// `${}` so [`crate::http::normalize_path`] reads it as a parameter. Returns `None` for a node
/// that is not a string literal. The bool is true when the value had an interpolation.
pub fn string_value(n: Node, src: &str) -> Option<(String, bool)> {
    match n.kind() {
        // JS/TS: "x" or 'x'; Python: "x", 'x', f"x{y}", r"x", triple quotes; Rust: "x".
        "string" | "string_literal" | "template_string" | "raw_string_literal" => {}
        _ => return None,
    }
    let mut out = String::new();
    let mut dynamic = false;
    let mut cursor = n.walk();
    let kids: Vec<Node> = n.named_children(&mut cursor).collect();
    let fragment_kinds = [
        "string_fragment",
        "string_content",
        "escape_sequence",
        "string_start",
        "string_end",
    ];
    let has_parts = kids.iter().any(|k| {
        fragment_kinds.contains(&k.kind())
            || k.kind() == "template_substitution"
            || k.kind() == "interpolation"
    });
    if !has_parts {
        // A literal with no parsed parts: strip prefix letters and quotes from the raw text.
        let raw = text(n, src);
        let raw = raw.trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '#');
        let q = raw.chars().next()?;
        if !matches!(q, '"' | '\'' | '`') {
            return None;
        }
        let triple: String = std::iter::repeat_n(q, 3).collect();
        let inner = if raw.starts_with(&triple) && raw.len() >= 6 {
            &raw[3..raw.len() - 3]
        } else if raw.len() >= 2 {
            &raw[1..raw.len() - 1]
        } else {
            ""
        };
        return Some((inner.trim_end_matches('#').to_string(), false));
    }
    for k in kids {
        match k.kind() {
            "string_fragment" | "string_content" | "escape_sequence" => out.push_str(text(k, src)),
            "template_substitution" | "interpolation" => {
                out.push_str("${}");
                dynamic = true;
            }
            _ => {}
        }
    }
    Some((out, dynamic))
}

/// The first ancestor (or the node itself) whose kind is in `kinds`.
pub fn ancestor<'t>(n: Node<'t>, kinds: &[&str]) -> Option<Node<'t>> {
    let mut cur = Some(n);
    while let Some(c) = cur {
        if kinds.contains(&c.kind()) {
            return Some(c);
        }
        cur = c.parent();
    }
    None
}

/// Visit every node in the tree, depth first, without recursion.
pub fn for_each_node<'t>(root: Node<'t>, mut f: impl FnMut(Node<'t>)) {
    let mut cursor = root.walk();
    loop {
        f(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_string(lang: Lang, src: &str) -> (String, bool) {
        let tree = parse(lang, src).expect("parse");
        let mut found = None;
        for_each_node(tree.root_node(), |n| {
            if found.is_none() {
                found = string_value(n, src);
            }
        });
        found.expect("a string literal")
    }

    #[test]
    fn every_grammar_parses() {
        let cases = [
            (Lang::Python, "def f(x):\n    return x\n"),
            (
                Lang::TypeScript,
                "export function f(x: number): number { return x }\n",
            ),
            (
                Lang::Tsx,
                "export const A = () => <button onClick={go}>Save</button>;\n",
            ),
            (Lang::JavaScript, "export function f(x) { return x }\n"),
            (
                Lang::Jsx,
                "export const A = () => <form onSubmit={go} />;\n",
            ),
            (Lang::Rust, "#[tauri::command]\nfn f(x: u32) -> u32 { x }\n"),
        ];
        for (lang, src) in cases {
            let tree = parse(lang, src).expect("grammar loads");
            assert!(!tree.root_node().has_error(), "{lang:?} parse error");
        }
    }

    #[test]
    fn string_values() {
        assert_eq!(
            first_string(Lang::Python, "x = '/users/{id}'\n"),
            ("/users/{id}".into(), false)
        );
        assert_eq!(
            first_string(Lang::Python, "x = f\"/users/{uid}/posts\"\n"),
            ("/users/${}/posts".into(), true)
        );
        assert_eq!(
            first_string(Lang::TypeScript, "fetch(`/api/users/${id}`)\n"),
            ("/api/users/${}".into(), true)
        );
        assert_eq!(
            first_string(Lang::JavaScript, "fetch(\"/api/x\")\n"),
            ("/api/x".into(), false)
        );
        assert_eq!(
            first_string(Lang::Rust, "fn f() { g(\"save_note\") }\n"),
            ("save_note".into(), false)
        );
    }

    #[test]
    fn langs() {
        assert_eq!(lang_of("a/b.tsx"), Some(Lang::Tsx));
        assert_eq!(lang_of("a/b.PY"), Some(Lang::Python));
        assert_eq!(lang_of("Makefile"), None);
        assert!(is_unsupported_source("svc/main.go"));
        assert!(!is_unsupported_source("README.md"));
    }
}
