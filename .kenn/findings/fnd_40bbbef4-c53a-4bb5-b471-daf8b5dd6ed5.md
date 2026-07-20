---
id: fnd_40bbbef4-c53a-4bb5-b471-daf8b5dd6ed5
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-11T10:17:28.6625Z
---
In kenn init, do not open the Store before handling the unparseable-config-without-force case. That branch must return early (non-zero exit, print the --force hint) before any Layout::resolve or Store::open, so a broken config leaves no .kenn scaffolding behind and a scripted init && index stops on the actionable hint instead of failing later.