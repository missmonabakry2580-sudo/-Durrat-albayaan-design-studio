use chrono::{DateTime, Utc};
use std::sync::Mutex;

/// The one piece of state that makes Mona's explicit instruction real: "any
/// step [Amin] wants to take must wait for me to say a confirming word
/// before it runs." When Claude asks for a `policy::RiskTier::ConfirmHighRisk`
/// tool, `commands::send_agent_message` stores it here *instead of* running
/// it, and returns a message asking Mona to approve or decline in her own
/// words. The very next call to `send_agent_message` checks this state
/// first — see `interpret` below for how her reply is read.
///
/// Deliberately holds at most one pending action: Amin never stacks up
/// several irreversible-ish proposals waiting for approval at once. A new
/// ConfirmHighRisk tool call overwrites whatever was pending before it (the
/// old one is implicitly abandoned, not silently executed later).
///
/// This is Mona's "Approval Token" — scoped to one action, target, and set
/// of parameters, carrying its own timestamp, and (see `is_expired`) good
/// for a limited window rather than an approval that could fire on a much
/// later, unrelated "ايوه" if the conversation moved on. It also already
/// ends the moment it's used: `commands.rs` clears `PendingConfirmation`'s
/// slot as part of executing or declining, never leaves it sitting armed.
#[derive(Clone)]
pub struct PendingAction {
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub proposed_at: DateTime<Utc>,
}

/// How long a proposed action stays approvable. Long enough that an
/// ordinary back-and-forth doesn't spuriously expire mid-conversation;
/// short enough that a stale proposal from a conversation Mona has since
/// moved on from can't be approved by surprise much later just because her
/// next message happens to contain an approval word for something else
/// entirely.
const APPROVAL_TIMEOUT_MINUTES: i64 = 10;

/// Whether `action` is too old to approve anymore — `commands.rs` checks
/// this before acting on any reply to a pending action, expiring it (and
/// telling Mona plainly) instead of executing a stale approval.
pub fn is_expired(action: &PendingAction, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(action.proposed_at) > chrono::Duration::minutes(APPROVAL_TIMEOUT_MINUTES)
}

pub struct PendingConfirmation(pub Mutex<Option<PendingAction>>);

impl PendingConfirmation {
    pub fn new() -> Self {
        PendingConfirmation(Mutex::new(None))
    }
}

impl Default for PendingConfirmation {
    fn default() -> Self {
        Self::new()
    }
}

pub enum Reply {
    Approve,
    Deny,
    /// Neither a clear yes nor a clear no — Amin must not guess. The caller
    /// re-states what's pending and waits again rather than treating
    /// silence or ambiguity as consent.
    Unclear,
}

const APPROVE_PHRASES: &[&str] = &["go ahead", "do it", "sounds good", "please proceed"];
const APPROVE_WORDS: &[&str] = &[
    "موافقة", "موافق", "موافقه", "نفذ", "نفذي", "ايوه", "أيوه", "ايوة", "أيوة", "تمام", "اه",
    "آه", "yes", "yep", "yeah", "confirm", "confirmed", "approved", "approve", "ok", "okay",
];
const DENY_PHRASES: &[&str] = &["لا تنفذ", "متنفذش", "متعملش", "don't do it", "do not"];
const DENY_WORDS: &[&str] = &[
    "لا", "لأ", "إلغاء", "الغاء", "كنسل", "no", "nope", "cancel", "stop", "deny", "declined",
];

/// Reads Mona's reply to a pending confirmation. Word-level matching (not
/// bare substring containment) so a normal sentence that happens to contain
/// "لا" inside a longer word, or "no" inside "note", doesn't misfire.
pub fn interpret(message: &str) -> Reply {
    let normalized = message.trim().to_lowercase();
    if normalized.is_empty() {
        return Reply::Unclear;
    }

    let words: Vec<String> = normalized
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect();

    let is_approve = APPROVE_PHRASES.iter().any(|p| normalized.contains(p))
        || words.iter().any(|w| APPROVE_WORDS.contains(&w.as_str()));
    let is_deny = DENY_PHRASES.iter().any(|p| normalized.contains(p))
        || words.iter().any(|w| DENY_WORDS.contains(&w.as_str()));

    match (is_approve, is_deny) {
        (true, false) => Reply::Approve,
        (false, true) => Reply::Deny,
        _ => Reply::Unclear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_proposed_at(proposed_at: DateTime<Utc>) -> PendingAction {
        PendingAction {
            tool_use_id: "toolu_1".to_string(),
            name: "delete_workspace_file".to_string(),
            input: serde_json::json!({}),
            proposed_at,
        }
    }

    #[test]
    fn a_fresh_proposal_is_not_expired() {
        let now = Utc::now();
        let action = action_proposed_at(now);
        assert!(!is_expired(&action, now));
    }

    #[test]
    fn a_proposal_just_under_the_timeout_is_not_expired() {
        let proposed_at = Utc::now();
        let now = proposed_at + chrono::Duration::minutes(APPROVAL_TIMEOUT_MINUTES - 1);
        assert!(!is_expired(&action_proposed_at(proposed_at), now));
    }

    #[test]
    fn a_proposal_past_the_timeout_is_expired() {
        let proposed_at = Utc::now();
        let now = proposed_at + chrono::Duration::minutes(APPROVAL_TIMEOUT_MINUTES + 1);
        assert!(is_expired(&action_proposed_at(proposed_at), now));
    }

    #[test]
    fn recognizes_arabic_approval_words() {
        assert!(matches!(interpret("موافقة"), Reply::Approve));
        assert!(matches!(interpret("طيب نفذ"), Reply::Approve));
        assert!(matches!(interpret("تمام كده"), Reply::Approve));
    }

    #[test]
    fn recognizes_english_approval_words() {
        assert!(matches!(interpret("yes"), Reply::Approve));
        assert!(matches!(interpret("please go ahead"), Reply::Approve));
    }

    #[test]
    fn recognizes_denial_in_both_languages() {
        assert!(matches!(interpret("لا"), Reply::Deny));
        assert!(matches!(interpret("إلغاء الأمر"), Reply::Deny));
        assert!(matches!(interpret("no, cancel that"), Reply::Deny));
    }

    #[test]
    fn does_not_misfire_on_words_that_merely_contain_a_trigger_substring() {
        // "لا" is a substring of many unrelated Arabic words; "no" of many
        // English ones. Word-boundary matching must not treat these as a
        // denial.
        assert!(matches!(interpret("لازم أراجع الملف الأول"), Reply::Unclear));
        assert!(matches!(interpret("please take note of this"), Reply::Unclear));
    }

    #[test]
    fn an_unrelated_message_is_unclear_not_a_guess() {
        assert!(matches!(interpret("إيه أخبار المدرسة النهاردة؟"), Reply::Unclear));
        assert!(matches!(interpret(""), Reply::Unclear));
    }

    #[test]
    fn a_message_with_both_signals_is_unclear_rather_than_picking_one() {
        assert!(matches!(interpret("yes but actually no"), Reply::Unclear));
    }
}
