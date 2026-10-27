//! The headline of a report that carries a computed verdict.
//!
//! [`crate::render_report`] shows the session's latest verdict record; [`crate::report::render_report_with`]
//! shows a verdict computed just now. Both reduce to a [`Head`].

use crate::ledger::{Ledger, LedgerError};
use crate::model::{Lane, Record, Tier, VerdictWord};
use crate::verdict::{Analysis, CheckState, Computed, is_blocking, word_label};

/// What the report's headline, three-state line and NOT ESTABLISHED list are built from.
#[derive(Debug, Clone)]
pub struct Head {
    /// The word.
    pub word: VerdictWord,
    /// Highest proof tier reached.
    pub tier: Option<Tier>,
    /// The lane, when known.
    pub lane: Option<Lane>,
    /// Policy version the verdict was computed under.
    pub policy_version: u32,
    /// What was not established.
    pub not_established: Vec<String>,
    /// Checkers the lane required (empty when read back from a stored verdict).
    pub required: Vec<String>,
    /// Record number of the stored verdict this was read from, if any.
    pub stored_seq: Option<u64>,
}

impl Head {
    /// From a verdict computed now.
    pub fn from_computed(c: &Computed) -> Head {
        Head {
            word: c.word,
            tier: c.highest_tier,
            lane: Some(c.lane),
            policy_version: c.policy_version,
            not_established: c.not_established.clone(),
            required: c.required.clone(),
            stored_seq: None,
        }
    }

    /// From the latest verdict record of the session, if there is one.
    pub fn from_stored(ledger: &Ledger, session: &str) -> Result<Option<Head>, LedgerError> {
        Ok(ledger
            .records_for_session(session)?
            .into_iter()
            .rev()
            .find_map(|s| match s.record {
                Record::Verdict {
                    word,
                    highest_tier,
                    not_established,
                    policy_version,
                    lane,
                    ..
                } => Some(Head {
                    word,
                    tier: highest_tier,
                    lane,
                    policy_version,
                    not_established,
                    required: Vec::new(),
                    stored_seq: Some(s.seq),
                }),
                _ => None,
            }))
    }

    /// The word as it reads, suffix included.
    pub fn label(&self) -> String {
        word_label(self.word, self.tier)
    }

    /// The headline's verdict text: the word, with a plain-words gloss for the two words that
    /// would otherwise be bare.
    pub fn headline_word(&self) -> String {
        match self.word {
            VerdictWord::Unverified => "UNVERIFIED - nothing was checked".to_string(),
            VerdictWord::Partial => "PARTIAL - a required check did not run".to_string(),
            _ => self.label(),
        }
    }
}

/// The three-state line, from the checks of the session.
pub fn three_state(a: &Analysis, lane: Lane) -> [String; 3] {
    let ran_at = |pred: &dyn Fn(Tier) -> bool| {
        a.checks
            .values()
            .any(|c| matches!(c, CheckState::Ran { tier, .. } if pred(*tier)))
    };
    let failing = |tier: Tier| {
        a.findings
            .iter()
            .any(|f| is_blocking(f, lane) && f.finding.tier == tier)
    };
    let pick = |ran: bool, tier: Tier, ok: &str, bad: &str| {
        if !ran {
            "not run".to_string()
        } else if failing(tier) {
            bad.to_string()
        } else {
            ok.to_string()
        }
    };
    [
        if ran_at(&|t| matches!(t, Tier::Static | Tier::Asset)) {
            "done".to_string()
        } else {
            "not run".to_string()
        },
        pick(
            ran_at(&|t| t == Tier::Ran),
            Tier::Ran,
            "ran, passed",
            "ran, failed",
        ),
        pick(
            ran_at(&|t| t == Tier::Browsed),
            Tier::Browsed,
            "browsed, passed",
            "browsed, failed",
        ),
    ]
}
