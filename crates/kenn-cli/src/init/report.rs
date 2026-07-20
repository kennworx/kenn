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
            Some(Availability::Degraded {
                command,
                hint,
                reason,
                not_executable,
            }) => {
                // "not found" and "ran and failed" need different fixes, so
                // they read differently. The old wording — "not runnable" —
                // covered both and helped with neither.
                let what = if *not_executable {
                    "not found"
                } else {
                    "ran and failed"
                };
                lines.push(format!(
                    "  {:<12} degraded → text fallback ({command} {what})",
                    spec.name
                ));
                // The indexer's own words WIN over the static hint. It knows
                // which dependency is missing; the hint can only name the tool,
                // and telling someone to install what they already have sends
                // them the wrong way. The hint remains the fallback, which is
                // all a third-party indexer ever has.
                if reason.is_empty() {
                    if !hint.is_empty() {
                        lines.push(format!("               install: {hint}"));
                    }
                } else {
                    for line in reason.lines().filter(|l| !l.trim().is_empty()) {
                        lines.push(format!("               {}", line.trim()));
                    }
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

    /// The whole point of capturing the probe's stderr. `kenn-swift` failing
    /// because `libIndexStore` cannot be loaded must NOT be reported as "install
    /// the Swift toolchain" — the toolchain is installed; a library from it is
    /// not on the load path, and only the captured message says so.
    #[test]
    fn a_degraded_language_shows_the_indexers_own_reason_over_the_hint() {
        let out = render(&[classified(
            "swift",
            Availability::Degraded {
                command: "kenn-swift".to_string(),
                hint: "install the Swift toolchain".to_string(),
                reason: "dyld[91]: Library not loaded: @rpath/libIndexStore.dylib".to_string(),
                not_executable: false,
            },
        )]);
        assert!(out.contains("libIndexStore"), "names the real cause: {out}");
        assert!(
            !out.contains("install the Swift toolchain"),
            "the generic hint must not displace the specific reason: {out}"
        );
        assert!(out.contains("ran and failed"), "{out}");
    }

    /// A third-party indexer produces no message, so the static hint is all
    /// there is — it must survive rather than being dropped along with the
    /// generic-hint path.
    #[test]
    fn an_absent_indexer_falls_back_to_the_static_hint() {
        let out = render(&[classified(
            "go",
            Availability::Degraded {
                command: "scip-go".to_string(),
                hint: "go install github.com/scip-code/scip-go/cmd/scip-go@latest".to_string(),
                reason: String::new(),
                not_executable: true,
            },
        )]);
        assert!(out.contains("install: go install"), "{out}");
        assert!(
            out.contains("not found"),
            "absent reads differently from failing: {out}"
        );
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
