//! Byte patterns with wildcards and captures, for finding code by its instructions.
//!
//! Syntax: hex bytes separated by spaces, `??` for any byte, `(` `)` around a capture.
//! `"a2 (?? ?? ?? ??) 50"` finds `mov [moffs], al; push eax` and captures the address.

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Pattern {
    bytes: Vec<Option<u8>>,
    /// Start of each capture, as an index into `bytes`.
    captures: Vec<usize>,
}

/// Where a pattern matched: offsets relative to the searched slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub len: usize,
    pub captures: Vec<usize>,
}

impl Pattern {
    /// Parses a pattern. Panics on bad syntax: patterns are constants in the source.
    pub fn new(text: &str) -> Self {
        let mut bytes = Vec::new();
        let mut captures = Vec::new();
        for tok in text.split_whitespace() {
            let mut tok = tok;
            if let Some(t) = tok.strip_prefix('(') {
                captures.push(bytes.len());
                tok = t;
            }
            let tok = tok.strip_suffix(')').unwrap_or(tok);
            bytes.push(match tok {
                "??" => None,
                hex => Some(
                    u8::from_str_radix(hex, 16)
                        .unwrap_or_else(|_| panic!("bad pattern byte {hex:?}")),
                ),
            });
        }
        assert!(!bytes.is_empty(), "empty pattern");
        Pattern { bytes, captures }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn matches_at(&self, hay: &[u8], at: usize) -> bool {
        self.bytes
            .iter()
            .zip(&hay[at..])
            .all(|(p, b)| p.is_none_or(|p| p == *b))
    }

    /// All non-overlapping matches, leftmost first.
    pub fn find_all(&self, hay: &[u8]) -> Vec<Match> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + self.bytes.len() <= hay.len() {
            if self.matches_at(hay, at) {
                out.push(Match {
                    start: at,
                    len: self.bytes.len(),
                    captures: self.captures.iter().map(|c| at + c).collect(),
                });
                at += self.bytes.len();
            } else {
                at += 1;
            }
        }
        out
    }

    /// The only match; `what` names it in the error when there are none or several.
    pub fn find_one(&self, hay: &[u8], what: &str) -> Result<Match> {
        let mut all = self.find_all(hay);
        match all.len() {
            1 => Ok(all.remove(0)),
            n => Err(Error(format!(
                "found {n} places that look like {what} (expected 1)"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_and_captures() {
        let p = Pattern::new("a2 (?? ?? ?? ??) 50");
        let hay = [0x90, 0xa2, 1, 2, 3, 4, 0x50, 0xa2, 5, 6, 7, 8, 0x51];
        let all = p.find_all(&hay);
        assert_eq!(
            all,
            vec![Match {
                start: 1,
                len: 6,
                captures: vec![2]
            }]
        );
        assert!(p.find_one(&hay, "x").is_ok());
        assert!(Pattern::new("a2").find_one(&hay, "a2").is_err());
    }

    #[test]
    fn non_overlapping() {
        assert_eq!(Pattern::new("aa aa").find_all(&[0xaa; 5]).len(), 2);
    }
}
