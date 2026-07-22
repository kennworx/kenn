---
id: fnd_ff0c150c-7f5d-4ef8-a0b9-b7de3373d76b
tags:
- directive
- polarity:dont
parent_ids: []
created_at: 2026-07-22T15:43:27.802617Z
---
cfg(windows) / cfg(not(unix)) code compiles ONLY on the ci-windows.yml gate — a macOS/Linux `cargo build`/clippy never touches it, so it rots silently against dependency updates. kenn-server is_alive/stop (pid.rs, runtime.rs) called windows-sys OpenProcess and tested `handle == 0`, which broke when windows-sys made HANDLE a `*mut c_void` (fix: `handle.is_null()`) — the compiler only caught it once kenn-store compiled far enough to reach kenn-server on the real Windows runner. The windows-support proposal wrongly assumed kenn-store held the ONLY Windows blockers; it did not. NEVER treat a green macOS/Linux run as evidence Windows-specific code compiles (design D5). When you add or touch any cfg(windows) code (kenn-server FFI, resolve.rs ancestor_device_id arm, atomic.rs rename_pointer), push and read the ci-windows result — it is the only verification.