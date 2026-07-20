//! The non-interactive report `kenn init` prints (task 4.1).
//!
//! One line per considered language — enabled, degraded, or absent — plus an
//! install hint for each failing probe, and a trailing summary. No stdin reads
//! and no TTY branch: `init` is step two of an agent's script, and a prompt
//! would hang it.

use super::detect::{Availability, Classified, SPECS};

/// Render the report for a set of classified languages. Absent languages (every
/// spec not in `classified`) are omitted, so the report shows only what's here.
#[must_use]
pub fn render(classified: &[Classified]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for spec in SPECS {
        let found = classified.iter().find(|c| c.spec.name == spec.name);
        match found.map(|c| &c.availability) {
            Some(Availability::Enabled) => {
                lines.push(format!("  {:<12} enabled", spec.name));
            }
            Some(Availability::Containerized { image }) => {
                lines.push(format!("  {:<12} containerized → {image}", spec.name));
            }
            Some(Availability::Degraded { command, hint }) => {
                lines.push(format!(
                    "  {:<12} degraded → text fallback ({command} not runnable)",
                    spec.name
                ));
                if !hint.is_empty() {
                    lines.push(format!("               install: {hint}"));
                }
            }
            None => {} // absent — omitted to keep the report to what's present
        }
    }

    let enabled = classified
        .iter()
        .filter(|c| c.availability == Availability::Enabled)
        .count();
    let containerized = classified
        .iter()
        .filter(|c| matches!(c.availability, Availability::Containerized { .. }))
        .count();
    let degraded = classified.len() - enabled - containerized;
    // Omit the containerized clause when there is none, so a non-`--docker` run
    // reports exactly as before.
    let summary = if containerized > 0 {
        format!("  {enabled} enabled, {containerized} containerized, {degraded} degraded")
    } else {
        format!("  {enabled} enabled, {degraded} degraded")
    };
    lines.push(summary);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::detect::SPECS;

    fn classified(name: &str, availability: Availability) -> Classified {
        let spec = SPECS.iter().find(|s| s.name == name).unwrap();
        Classified { spec, availability }
    }

    #[test]
    fn containerized_language_renders_image_and_is_counted() {
        let out = render(&[
            classified("rust", Availability::Enabled),
            classified(
                "go",
                Availability::Containerized {
                    image: "ghcr.io/kennworx/kenn-go@sha256:abc".to_string(),
                },
            ),
        ]);
        assert!(
            out.contains("containerized → ghcr.io/kennworx/kenn-go@sha256:abc"),
            "{out}"
        );
        assert!(
            out.contains("1 enabled, 1 containerized, 0 degraded"),
            "{out}"
        );
    }

    #[test]
    fn summary_omits_containerized_clause_when_none() {
        let out = render(&[classified("rust", Availability::Enabled)]);
        assert!(out.contains("1 enabled, 0 degraded"), "{out}");
        assert!(!out.contains("containerized"), "{out}");
    }
}
