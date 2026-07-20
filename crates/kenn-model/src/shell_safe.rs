//! The shell-safe alphabet — the one piece of the shell-safe-symbol-ids contract
//! (design D6) that must be **shared**, because two crates have to agree on it:
//! the *producer* (`kenn_indexer::pubid`, which renders every `pub_id`) and the
//! *enforcer* (`kenn_store`'s writer, which `debug_assert`s that no stored
//! `pub_id` contains an unsafe byte).
//!
//! A `pub_id` is handed to the shell as a `kenn get <pub_id>` argument, so it must
//! be a single unquoted POSIX-shell-safe token. The *rendering* (how each language
//! turns its descriptors into a safe id, and how residual hostile bytes are
//! floored) lives entirely in `kenn_indexer::pubid` — it is per-language and not
//! this crate's concern. This module only answers "is this char safe?".
//!
//! Safe alphabet: `A–Z a–z 0–9` and other Unicode alphanumerics (literal to the
//! shell) plus `. _ / : @ + , = ~ # -`. The `rs:`/`ts:`/… prefix neutralizes the
//! word-start caveats on `# ~ -`.

/// Whether `ch` may appear literally in a shell-safe `pub_id`. The single source
/// of truth the id producer and the store enforcer both consult.
#[must_use]
pub fn is_safe(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            '.' | '_' | '/' | ':' | '@' | '+' | ',' | '=' | '~' | '#' | '-'
        )
}

#[cfg(test)]
mod tests {
    use super::is_safe;

    #[test]
    fn safe_alphabet_matches_the_contract() {
        // Alphanumerics (ASCII + Unicode) and the delimiter set are safe.
        for c in "azAZ09._/:@+,=~#-щ".chars() {
            assert!(is_safe(c), "{c:?} should be safe");
        }
        // Shell metacharacters are not.
        for c in " !\"$%&'()*;<>?[\\]^`{|}".chars() {
            assert!(!is_safe(c), "{c:?} should be unsafe");
        }
    }
}
