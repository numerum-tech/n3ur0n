//! Lobe identifiers and the instance-level membership rules.
//!
//! A lobe is a named federation of instances (architecture §3, §6). An instance
//! declares the lobes it belongs to in `describe_self`; a capability may only
//! claim a lobe its own instance declares — `cap.lobe_ids ⊆ instance.lobe_ids`.
//! The subset rule is enforced where lobes are **written** (manifest upsert,
//! registry load), so readers can take the declared set at face value.
//!
//! Membership itself is not verified before v0.5: declaring a lobe is a claim,
//! not a proof. Lobe scoping is a retrieval filter, never a trust boundary.

use crate::error::{CoreError, CoreResult};

/// Maximum number of lobes a single instance may declare. A gateway that
/// belongs everywhere belongs nowhere: the cap keeps the claim meaningful and
/// bounds the `describe_self` payload.
pub const MAX_LOBES_PER_INSTANCE: usize = 5;

/// Shortest accepted lobe id.
pub const LOBE_ID_MIN_LEN: usize = 2;

/// Longest accepted lobe id.
pub const LOBE_ID_MAX_LEN: usize = 64;

/// Validate one lobe id.
///
/// Grammar: lowercase alphanumerics at both ends, with `-` and `.` allowed
/// inside, 2 to 64 characters. Both the short form (`medical`) and the
/// namespaced form documented in the manifest spec
/// (`lobe.community.translators.v1`) are accepted.
///
/// Case is **not** normalised: an id that is not already lowercase is rejected,
/// so two spellings of the same lobe can never denote two different lobes.
pub fn validate_lobe_id(id: &str) -> CoreResult<()> {
    let n = id.chars().count();
    if n < LOBE_ID_MIN_LEN {
        return Err(CoreError::InvalidIdentifier(format!(
            "lobe id `{id}` is shorter than {LOBE_ID_MIN_LEN} characters"
        )));
    }
    if n > LOBE_ID_MAX_LEN {
        return Err(CoreError::InvalidIdentifier(format!(
            "lobe id is longer than {LOBE_ID_MAX_LEN} characters"
        )));
    }
    for c in id.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') {
            return Err(CoreError::InvalidIdentifier(format!(
                "lobe id `{id}` contains `{c}`; allowed: a-z, 0-9, `-`, `.`"
            )));
        }
    }
    let first = id.chars().next().unwrap_or_default();
    let last = id.chars().last().unwrap_or_default();
    if !(first.is_ascii_lowercase() || first.is_ascii_digit())
        || !(last.is_ascii_lowercase() || last.is_ascii_digit())
    {
        return Err(CoreError::InvalidIdentifier(format!(
            "lobe id `{id}` must start and end with a letter or a digit"
        )));
    }
    Ok(())
}

/// Validate the full set an instance declares: every id well-formed, no
/// duplicate, at most [`MAX_LOBES_PER_INSTANCE`].
pub fn validate_instance_lobes(ids: &[String]) -> CoreResult<()> {
    if ids.len() > MAX_LOBES_PER_INSTANCE {
        return Err(CoreError::InvalidIdentifier(format!(
            "an instance may declare at most {MAX_LOBES_PER_INSTANCE} lobes, got {}",
            ids.len()
        )));
    }
    for (i, id) in ids.iter().enumerate() {
        validate_lobe_id(id)?;
        if ids[..i].contains(id) {
            return Err(CoreError::InvalidIdentifier(format!(
                "lobe id `{id}` is declared twice"
            )));
        }
    }
    Ok(())
}

/// The lobes a capability claims but its instance does not declare.
///
/// Empty slice means the capability satisfies `cap.lobe_ids ⊆
/// instance.lobe_ids`. An instance that declares no lobe therefore rejects
/// every lobe claim, which is the intended reading: a capability cannot put
/// its gateway in a federation the gateway itself did not join.
pub fn unclaimable_lobes<'a>(cap_lobes: &'a [String], instance_lobes: &[String]) -> Vec<&'a str> {
    cap_lobes
        .iter()
        .filter(|l| !instance_lobes.contains(l))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_short_and_namespaced_forms() {
        validate_lobe_id("medical").unwrap();
        validate_lobe_id("lobe.community.translators.v1").unwrap();
        validate_lobe_id("legal-fr").unwrap();
        validate_lobe_id("a1").unwrap();
    }

    #[test]
    fn rejects_bad_charset_case_and_edges() {
        assert!(validate_lobe_id("Medical").is_err());
        assert!(validate_lobe_id("med ical").is_err());
        assert!(validate_lobe_id("med_ical").is_err());
        assert!(validate_lobe_id("-medical").is_err());
        assert!(validate_lobe_id("medical.").is_err());
        assert!(validate_lobe_id("m").is_err());
        assert!(validate_lobe_id(&"a".repeat(LOBE_ID_MAX_LEN + 1)).is_err());
    }

    #[test]
    fn instance_set_is_bounded_and_duplicate_free() {
        let five: Vec<String> = (0..MAX_LOBES_PER_INSTANCE)
            .map(|i| format!("lobe{i}"))
            .collect();
        validate_instance_lobes(&five).unwrap();

        let six: Vec<String> = (0..=MAX_LOBES_PER_INSTANCE)
            .map(|i| format!("lobe{i}"))
            .collect();
        assert!(validate_instance_lobes(&six).is_err());

        let dup = vec!["medical".to_string(), "medical".to_string()];
        assert!(validate_instance_lobes(&dup).is_err());
    }

    #[test]
    fn subset_rule_reports_every_unclaimable_lobe() {
        let instance = vec!["medical".to_string(), "legal-fr".to_string()];
        let cap = vec!["medical".to_string(), "finance".to_string()];
        assert_eq!(unclaimable_lobes(&cap, &instance), vec!["finance"]);
        assert!(unclaimable_lobes(&[], &instance).is_empty());
        assert_eq!(unclaimable_lobes(&cap, &[]), vec!["medical", "finance"]);
    }
}
