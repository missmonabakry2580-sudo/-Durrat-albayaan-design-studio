use serde_json::Value;

/// Mona's explicit Verification Layer requirement: the word "تم" ("done")
/// may only ever describe a result Amin has actually checked, never one it
/// merely planned or attempted. Before this existed as its own module, the
/// same `if exec_result.is_ok() { "تم: ..." } else { "خطأ أثناء: ..." }`
/// branch was duplicated at both call sites in `commands.rs` (the immediate
/// Auto/TrustedDelegation path and the post-approval ConfirmHighRisk path)
/// — pulling it out here means there is exactly one place this rule can be
/// gotten wrong, and it's a rule this file's tests can check directly rather
/// than depending on integration coverage of `commands.rs`'s Tauri command
/// plumbing.
///
/// This only covers Amin's own local tool executions (`tools::execute`'s
/// real, synchronous result) — a real external connector (email sent,
/// calendar event created) will need its own verification step once one
/// exists, since a successful API call there doesn't necessarily mean the
/// real-world effect Mona wanted actually happened; that's flagged as
/// future work, not solved by this function.
pub fn verified_outcome(result: &Result<Value, String>, description: &str) -> String {
    match result {
        Ok(_) => format!("تم: {description}"),
        Err(e) => format!("حصل خطأ أثناء: {description} — {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_real_success_is_reported_as_done() {
        let result: Result<Value, String> = Ok(json!({ "ok": true }));
        assert_eq!(verified_outcome(&result, "إضافة مهمة: x"), "تم: إضافة مهمة: x");
    }

    #[test]
    fn a_real_failure_is_never_reported_as_done() {
        let result: Result<Value, String> = Err("no task found with id x".to_string());
        let outcome = verified_outcome(&result, "تغيير حالة المهمة x");
        assert!(!outcome.starts_with("تم:"), "a failed result must never be reported as تم");
        assert!(outcome.contains("تغيير حالة المهمة x"));
        assert!(outcome.contains("no task found with id x"));
    }
}
