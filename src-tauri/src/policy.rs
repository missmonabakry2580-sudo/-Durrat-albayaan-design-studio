use serde::{Deserialize, Serialize};

/// The three permission tiers from the brief, plus `Excluded` for domains
/// Amin must never touch at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTier {
    Auto,
    TrustedDelegation,
    ConfirmHighRisk,
    Excluded,
}

impl RiskTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskTier::Auto => "auto",
            RiskTier::TrustedDelegation => "trusted_delegation",
            RiskTier::ConfirmHighRisk => "confirm_high_risk",
            RiskTier::Excluded => "excluded",
        }
    }
}

/// Observe/Assist/Delegate/Autopilot — how much Amin is allowed to act
/// without a human in the loop. Independent from `RiskTier`: a high
/// autonomy level still never overrides `Excluded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyLevel {
    Observe,
    Assist,
    Delegate,
    Autopilot,
}

impl AutonomyLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            AutonomyLevel::Observe => "observe",
            AutonomyLevel::Assist => "assist",
            AutonomyLevel::Delegate => "delegate",
            AutonomyLevel::Autopilot => "autopilot",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "observe" => Ok(AutonomyLevel::Observe),
            "assist" => Ok(AutonomyLevel::Assist),
            "delegate" => Ok(AutonomyLevel::Delegate),
            "autopilot" => Ok(AutonomyLevel::Autopilot),
            other => Err(format!("unknown autonomy level: {other}")),
        }
    }
}

impl Default for AutonomyLevel {
    /// Amin starts in Observe until the user explicitly turns on more
    /// autonomy — never the other way around.
    fn default() -> Self {
        AutonomyLevel::Observe
    }
}

/// Domains Amin must never act in, regardless of autonomy level or any
/// runtime instruction — including one that arrives disguised as a user
/// message via injected content. This is the one place that decides
/// "banking" is off-limits; it is not left to prompting alone. Expand this
/// list, never relax it, as later phases add real tools.
const EXCLUDED_DOMAINS: &[&str] = &["banking", "payment", "wire_transfer", "investment_trading"];

/// Irreversible or externally-visible actions: always confirm with the user
/// first, no matter the autonomy level.
const HIGH_RISK_KEYWORDS: &[&str] = &["send_email", "post_public", "delete", "purchase"];

/// Reversible, low-blast-radius actions Delegate-level autonomy may take
/// on its own, subject to the Follow-up Engine and audit log.
const DELEGATED_KEYWORDS: &[&str] = &["draft", "schedule", "reminder", "research"];

/// Classify an action domain (e.g. "send_email", "draft_reminder") into a
/// risk tier. This is a stub for Phase 0 — each later phase registers its
/// real tool names here as it adds them, rather than inventing ad hoc
/// checks at the call site.
pub fn classify(domain: &str) -> RiskTier {
    let d = domain.to_lowercase();
    if EXCLUDED_DOMAINS.iter().any(|kw| d.contains(kw)) {
        RiskTier::Excluded
    } else if HIGH_RISK_KEYWORDS.iter().any(|kw| d.contains(kw)) {
        RiskTier::ConfirmHighRisk
    } else if DELEGATED_KEYWORDS.iter().any(|kw| d.contains(kw)) {
        RiskTier::TrustedDelegation
    } else {
        RiskTier::Auto
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_each_tier_by_keyword() {
        assert_eq!(classify("banking_transfer"), RiskTier::Excluded);
        assert_eq!(classify("send_email"), RiskTier::ConfirmHighRisk);
        assert_eq!(classify("draft_reminder"), RiskTier::TrustedDelegation);
        assert_eq!(classify("check_weather"), RiskTier::Auto);
    }

    #[test]
    fn classify_is_case_insensitive() {
        assert_eq!(classify("SEND_EMAIL"), RiskTier::ConfirmHighRisk);
        assert_eq!(classify("Banking_Login"), RiskTier::Excluded);
    }

    #[test]
    fn excluded_wins_even_if_a_high_risk_keyword_also_matches() {
        // A hypothetical "delete_banking_data" domain must never slip
        // through as merely "confirm" — Excluded is checked first and
        // must stay that way as more keywords are added later.
        assert_eq!(classify("delete_banking_data"), RiskTier::Excluded);
    }

    #[test]
    fn autonomy_level_parse_round_trips_with_as_str() {
        for level in [
            AutonomyLevel::Observe,
            AutonomyLevel::Assist,
            AutonomyLevel::Delegate,
            AutonomyLevel::Autopilot,
        ] {
            assert_eq!(AutonomyLevel::parse(level.as_str()).unwrap(), level);
        }
    }

    #[test]
    fn autonomy_level_parse_rejects_unknown_input() {
        assert!(AutonomyLevel::parse("god-mode").is_err());
    }

    #[test]
    fn risk_tier_as_str_matches_its_serde_wire_format() {
        // commands::classify_action sends `as_str()` straight to the
        // frontend; if this ever drifted from the derived Serialize
        // output, the frontend's RiskTier union type would silently stop
        // matching what the backend actually sends.
        for tier in [
            RiskTier::Auto,
            RiskTier::TrustedDelegation,
            RiskTier::ConfirmHighRisk,
            RiskTier::Excluded,
        ] {
            let json = serde_json::to_string(&tier).unwrap();
            assert_eq!(json, format!("\"{}\"", tier.as_str()));
        }
    }
}
