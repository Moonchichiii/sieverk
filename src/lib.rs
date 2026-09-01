//! sieverk — SIE engine library.
//!
//! Parsers tolerate, the validator judges, and nothing in here touches a
//! database or the network. The binary in `main.rs` is only a thin CLI
//! over this crate.

use codepage_437::{FromCp437, CP437_CONTROL};

pub mod accounts;
pub mod metadata;
pub mod money;
pub mod snapshot;
pub mod tokenizer;
pub mod validator;
pub mod vouchers;

/// SIE files declare `#FORMAT PC8`, which means IBM codepage 437 — a DOS-era
/// encoding. Reading them as UTF-8 turns å/ä/ö into mojibake, so decoding
/// is step zero, before any parsing.
///
/// (Real-world caveat: some exporters lie and emit ISO-8859-1 or UTF-8 anyway.
/// A robust reader eventually sniffs before assuming. Later problem.)
pub fn decode_sie_bytes(bytes: Vec<u8>) -> String {
    String::from_cp437(bytes, &CP437_CONTROL)
}
