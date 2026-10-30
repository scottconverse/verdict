//! The command line's check that a person makes a human decision.
//!
//! `--i-am-the-human` alone is a flag that any program can type. So a human decision made from the
//! command line also needs a person at a terminal who types `yes`. An agent's shell tool normally
//! has no terminal on standard input, so the decision is refused there and nothing is recorded.
//!
//! This raises the bar. It is not proof: a program that opens a pseudo-terminal can still type
//! `yes`. The page's buttons, behind the one-time link, are the other human surface.

use std::io::{BufRead, IsTerminal, Write};

/// Ask the person at the terminal to type `yes` to `what`. True only when they did.
pub fn confirm(what: &str) -> bool {
    // Debug builds only (the test suite): this variable stands for what the person typed.
    // A release build ignores it.
    #[cfg(debug_assertions)]
    if let Ok(typed) = std::env::var("VDT_TEST_TYPED") {
        return said_yes(what, &typed);
    }
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        eprintln!(
            "Refused: \"{what}\" is a decision only a person makes. vdt needs you at a terminal to \
             type yes, and this command has no terminal (an agent's shell usually has none). \
             Nothing was changed. Run the command in your own terminal window, or use the button \
             on the Verdict page."
        );
        return false;
    }
    eprint!("{what}.\nType yes and press Enter to go ahead: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        line.clear();
    }
    said_yes(what, &line)
}

fn said_yes(what: &str, typed: &str) -> bool {
    let yes = typed.trim().eq_ignore_ascii_case("yes");
    if !yes {
        eprintln!("Nothing was changed: you did not type yes to \"{what}\".");
    }
    yes
}

#[cfg(test)]
mod tests {
    use super::said_yes;

    #[test]
    fn only_the_word_yes_counts() {
        assert!(said_yes("x", "yes\n"));
        assert!(said_yes("x", "  YES \r\n"));
        assert!(!said_yes("x", ""));
        assert!(!said_yes("x", "y"));
        assert!(!said_yes("x", "yes please"));
    }
}
