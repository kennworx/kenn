---
id: fnd_eaaf8c6f-f17c-4b52-bc84-450f6ebfe317
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-15T15:39:54.509859Z
---
Do C# package/assembly naming in the .NET sidecar (Roslyn AssemblyName), never by re-parsing .csproj XML in the Rust PackageLayout. The Rust marker path is only a directory-name fallback and must not duplicate sidecar naming logic.