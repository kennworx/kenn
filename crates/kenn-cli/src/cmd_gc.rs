//! `kenn gc` — evict least-recently-used vector-cache generations past
//! the `[vectors] cache_cap_mb` size cap. The active generation (the one
//! the configured model reads/writes) and any directory holding committed
//! `pack-*.bin` files are never evicted. The same pass also runs lazily at
//! the start of every embed pass; this command is the explicit trigger.

use anyhow::Result;
use kenn_store::Layout;

use crate::exit::ExitCodes;

pub fn run(layout: &Layout, model_id: &str) -> Result<ExitCodes> {
    let cap_mb = layout.vectors_cache_cap_mb();
    if cap_mb == 0 {
        println!("vector-cache GC is disabled ([vectors] cache_cap_mb = 0)");
        return Ok(ExitCodes::Ok);
    }
    let report = kenn_store::gc_vector_cache(layout, model_id, cap_mb)?;
    for dir in &report.evicted {
        println!("evicted {}", dir.display());
    }
    if report.evicted.is_empty() {
        println!(
            "vector cache within its {cap_mb} MiB cap ({} KiB used); nothing evicted",
            report.remaining_bytes / 1024
        );
    } else {
        println!(
            "freed {} KiB; {} KiB remain (cap {cap_mb} MiB)",
            report.freed_bytes / 1024,
            report.remaining_bytes / 1024
        );
    }
    Ok(ExitCodes::Ok)
}
