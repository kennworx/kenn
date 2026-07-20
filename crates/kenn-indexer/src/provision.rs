//! Optional `Directory.Build.props` provisioning (section 11 of the proposal).
//!
//! Solves the C# indexing pain point where workspaces lacking a top-level
//! `Directory.Build.props` cause `MSBuildWorkspace` to surface `NuGet`
//! vulnerability errors during indexing (`kenn-dotnet` exits non-zero).
//!
//! Strict rules:
//! * Never modify an existing file — return `AlreadyExists`.
//! * Gated behind `kenn.toml` `[language.csharp] provision_directory_build_props = true`
//!   AND an explicit caller signal (CLI flag / interactive consent). The
//!   caller's job to honor the gate; this module just executes the write.

use std::path::Path;

use crate::report::ProvisionResult;

const PROVISIONED_CONTENTS: &str = r"<Project>
  <!--
    Auto-provisioned by `kenn`. Suppresses NuGet vulnerability errors
    during C# indexing without changing project compile behavior.
    Delete or edit freely; `kenn` will not overwrite an existing file.
  -->
  <PropertyGroup>
    <NuGetAuditMode>direct</NuGetAuditMode>
    <NuGetAuditLevel>high</NuGetAuditLevel>
    <WarningsNotAsErrors>$(WarningsNotAsErrors);NU1901;NU1902;NU1903;NU1904</WarningsNotAsErrors>
  </PropertyGroup>
</Project>
";

/// Provision a `Directory.Build.props` at the workspace root, but only if
/// none exists. Caller MUST have already validated the config gate +
/// user consent.
pub fn provision_csharp_directory_build_props(
    workspace_root: &Path,
) -> std::io::Result<ProvisionResult> {
    let target = workspace_root.join("Directory.Build.props");
    if target.exists() {
        return Ok(ProvisionResult::AlreadyExists);
    }
    std::fs::write(&target, PROVISIONED_CONTENTS)?;
    Ok(ProvisionResult::Created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let result = provision_csharp_directory_build_props(dir.path()).unwrap();
        assert_eq!(result, ProvisionResult::Created);
        let body = std::fs::read_to_string(dir.path().join("Directory.Build.props")).unwrap();
        assert!(body.contains("NuGetAuditMode"));
    }

    #[test]
    fn never_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Directory.Build.props");
        std::fs::write(&target, "<Project>USER</Project>").unwrap();
        let result = provision_csharp_directory_build_props(dir.path()).unwrap();
        assert_eq!(result, ProvisionResult::AlreadyExists);
        // File contents preserved.
        let body = std::fs::read_to_string(&target).unwrap();
        assert_eq!(body, "<Project>USER</Project>");
    }
}
