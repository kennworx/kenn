//! One-time process registration of the `sqlite-vec` loadable extension.
//!
//! `sqlite3_auto_extension` installs `sqlite3_vec_init` to run on every new
//! `SQLite` connection, so `vec0` virtual tables are available without a
//! per-connection load step. Must run before the first connection opens;
//! [`ensure_vec_extension`] is idempotent via a `Once`.

use std::sync::Once;

static REGISTER: Once = Once::new();

/// Register `sqlite-vec` with `SQLite`'s auto-extension hook, once per process.
/// Called before opening any connection that touches a `vec0` table.
pub(crate) fn ensure_vec_extension() {
    REGISTER.call_once(|| {
        #[expect(
            unsafe_code,
            reason = "FFI: install sqlite-vec's init fn via sqlite3_auto_extension"
        )]
        #[expect(
            clippy::missing_transmute_annotations,
            reason = "cast mirrors sqlite-vec's documented rusqlite registration"
        )]
        #[expect(
            clippy::multiple_unsafe_ops_per_block,
            reason = "the transmute and the sqlite3_auto_extension call are one indivisible registration step"
        )]
        // SAFETY: `sqlite3_vec_init` is a valid C-ABI extension entry point of the
        // shape `sqlite3_auto_extension` expects; registered once before any
        // connection opens.
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}
