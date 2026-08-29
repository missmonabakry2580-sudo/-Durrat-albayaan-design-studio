use chrono::Utc;
use rusqlite::{params, Connection};
use uuid::Uuid;

use crate::policy::RiskTier;

#[derive(Debug, Clone, Copy)]
pub enum Decision {
    Executed,
    Confirmed,
    Declined,
    Blocked,
    /// Amin has asked to take a ConfirmHighRisk action and is now waiting
    /// on Mona's explicit word ("موافقة" / "نفذ" / an explicit "no") before
    /// it runs. See commands::send_agent_message and confirmation.rs.
    Proposed,
}

impl Decision {
    fn as_str(&self) -> &'static str {
        match self {
            Decision::Executed => "executed",
            Decision::Confirmed => "confirmed",
            Decision::Declined => "declined",
            Decision::Blocked => "blocked",
            Decision::Proposed => "proposed",
        }
    }
}

/// Append one row to the audit log. There is deliberately no matching
/// `update`/`delete` in this module — the log is append-only by
/// construction, not by convention.
pub fn record(
    conn: &Connection,
    actor: &str,
    action: &str,
    risk_tier: RiskTier,
    decision: Decision,
    details: Option<&str>,
    evidence: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO audit_log (id, ts, actor, action, risk_tier, decision, details, evidence)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            Utc::now().to_rfc3339(),
            actor,
            action,
            risk_tier.as_str(),
            decision.as_str(),
            details,
            evidence,
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
