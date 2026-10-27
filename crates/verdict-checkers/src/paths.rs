//! Path classification shared by the checkers. Pure string rules; nothing here touches the disk.

/// Folders never walked: tool state, version control and build output (the same list the
/// baseline snapshot skips).
pub const SKIP_DIRS: &[&str] = &[
    ".verdict",
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
];

/// Lower-case extension of the last path component, or `""`.
pub fn ext(path: &str) -> String {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The last path component.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The file name without its last extension.
pub fn stem(path: &str) -> String {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_string(),
        _ => name.to_string(),
    }
}

/// The folder part of a path (`""` for a top-level file).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(p, _)| p).unwrap_or("")
}

/// Normalize a path to `/` separators with no leading `./`.
pub fn normalize(path: &str) -> String {
    let p = path.replace('\\', "/");
    let mut p = p.as_str();
    while let Some(rest) = p.strip_prefix("./") {
        p = rest;
    }
    p.to_string()
}

/// Source-code extensions the line rules read.
const CODE_EXTS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "mjs", "cjs", "py", "go", "java", "kt", "cs", "rb", "php",
    "swift", "c", "h", "cpp", "hpp", "sh", "ps1", "vue", "svelte", "sql",
];

/// True for a source-code file the line rules read.
pub fn is_code(path: &str) -> bool {
    CODE_EXTS.contains(&ext(path).as_str())
}

/// JavaScript / TypeScript family.
pub fn is_js_like(path: &str) -> bool {
    matches!(
        ext(path).as_str(),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "vue" | "svelte"
    )
}

/// True when the path alone says it is a test file.
pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = file_name(&lower);
    if lower
        .split('/')
        .rev()
        .skip(1)
        .any(|c| matches!(c, "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e"))
    {
        return true;
    }
    name.starts_with("test_")
        || name.contains(".test.")
        || name.contains(".spec.")
        || stem(name).ends_with("_test")
        || stem(name).ends_with("_spec")
        || has_upper_test_suffix(path)
}

fn has_upper_test_suffix(path: &str) -> bool {
    // FooTest.java, FooTest.kt, FooTests.cs: a capital T so `latest.rs` does not count.
    let s = stem(file_name(path));
    s.ends_with("Test") || s.ends_with("Tests")
}

/// Files whose lines nobody wrote by hand: locks, minified bundles, vendored or generated code.
pub fn is_noise_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = file_name(&lower);
    if lower.starts_with(".verdict/") || lower.contains("/.verdict/") {
        return true;
    }
    if matches!(
        name,
        "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "cargo.lock"
            | "poetry.lock"
            | "pipfile.lock"
            | "composer.lock"
            | "go.sum"
            | "gemfile.lock"
    ) {
        return true;
    }
    if name.ends_with(".min.js") || name.ends_with(".min.css") || name.ends_with(".map") {
        return true;
    }
    lower.split('/').any(|c| {
        matches!(
            c,
            "node_modules" | "vendor" | "generated" | "dist" | "build" | "target"
        )
    })
}

/// True when the policy's `exclude` list names this path: an entry matches a whole path
/// component or a leading folder path (`vendor` or `src/vendor`).
pub fn is_excluded(path: &str, exclude: &[String]) -> bool {
    exclude.iter().any(|e| {
        let e = e.trim_matches('/');
        !e.is_empty()
            && (path == e
                || path.starts_with(&format!("{e}/"))
                || path.split('/').any(|c| c == e)
                || path.contains(&format!("/{e}/")))
    })
}

/// Case-insensitive "does `hay` contain `needle` as a whole word-ish token". Used to decide
/// whether a human-written sentence mentions a name.
pub fn mentions(hay_lower: &str, needle: &str) -> bool {
    let n = needle.to_ascii_lowercase();
    !n.is_empty() && hay_lower.contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_are_recognized() {
        for p in [
            "tests/a.rs",
            "src/foo.test.ts",
            "src/foo.spec.js",
            "pkg/test_foo.py",
            "pkg/foo_test.py",
            "x/FooTest.java",
            "a/b/foo_test.go",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in [
            "src/latest.rs",
            "src/contest.rs",
            "src/main.rs",
            "src/protest.py",
        ] {
            assert!(!is_test_path(p), "{p}");
        }
    }

    #[test]
    fn excluded_matches_components() {
        let ex = vec!["vendor".to_string(), "src/gen".to_string()];
        assert!(is_excluded("vendor/a.js", &ex));
        assert!(is_excluded("a/vendor/b.js", &ex));
        assert!(is_excluded("src/gen/x.ts", &ex));
        assert!(!is_excluded("src/vendors.ts", &ex));
    }
}
