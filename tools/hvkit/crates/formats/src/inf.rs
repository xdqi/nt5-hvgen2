//! Windows setup text files (TXTSETUP.SIF, WINNT.SIF, HIVESYS.INF and other INFs), edited without
//! disturbing anything else in them.
//!
//! An 8-bit file is held one char per byte (ISO 8859-1), so text in any ANSI code page (GBK on the
//! zh-hans CDs) passes through unchanged; a file with a UTF-16LE byte order mark stays UTF-16LE. Lines
//! are split at CR LF and joined with CR LF, so line ends, a missing last line end and blank lines are
//! kept as they were. Section and key names compare case-insensitively.

use crate::{Error, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// One byte per char.
    Latin1,
    /// UTF-16LE with a byte order mark.
    Utf16,
}

#[derive(Debug, Clone)]
pub struct Text {
    pub lines: Vec<String>,
    pub encoding: Encoding,
}

/// The key of a `key = value` line, lower case; None for comments and lines without '='.
pub fn key(line: &str) -> Option<String> {
    if !line.contains('=') || line.trim_start().starts_with(';') {
        return None;
    }
    Some(line.split('=').next().unwrap_or("").trim().to_lowercase())
}

/// The name of a section header line (`[name]`, possibly indented), as written.
pub fn header(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('[')?;
    let end = rest.find(']')?;
    (end > 0).then(|| &rest[..end])
}

impl Text {
    pub fn parse(bytes: &[u8]) -> Result<Text> {
        let (text, encoding) = match bytes.strip_prefix(&[0xff, 0xfe]) {
            Some(rest) => {
                if rest.len() % 2 != 0 {
                    bail!("odd length for a UTF-16 file");
                }
                let w: Vec<u16> = rest
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                (
                    String::from_utf16(&w).map_err(|_| Error("invalid UTF-16".into()))?,
                    Encoding::Utf16,
                )
            }
            None => (
                bytes.iter().map(|&b| char::from(b)).collect(),
                Encoding::Latin1,
            ),
        };
        Ok(Text {
            lines: text.split("\r\n").map(String::from).collect(),
            encoding,
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let text = self.lines.join("\r\n");
        match self.encoding {
            Encoding::Utf16 => Ok([0xff, 0xfe]
                .into_iter()
                .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
                .collect()),
            Encoding::Latin1 => text
                .chars()
                .map(|c| {
                    u8::try_from(u32::from(c))
                        .map_err(|_| Error(format!("{c:?} does not fit an 8-bit file")))
                })
                .collect(),
        }
    }

    /// Every occurrence of every section: (lower-case name, header index, end), where the body is
    /// header+1..end and end is just after its last non-blank line.
    pub fn sections(&self) -> Vec<(String, usize, usize)> {
        let mut out: Vec<(String, usize, usize)> = Vec::new();
        for (i, l) in self.lines.iter().enumerate() {
            if let Some(name) = header(l) {
                out.push((name.to_lowercase(), i, i + 1));
            } else if !l.trim().is_empty()
                && let Some(cur) = out.last_mut()
            {
                cur.2 = i + 1;
            }
        }
        out
    }

    /// The body (start, end) of the first section `name`.
    pub fn section(&self, name: &str) -> Option<(usize, usize)> {
        let name = name.to_lowercase();
        self.sections()
            .into_iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, h, e)| (h + 1, e))
    }

    fn require(&self, name: &str) -> Result<(usize, usize)> {
        self.section(name)
            .ok_or_else(|| Error(format!("section [{name}] not found")))
    }

    /// Appends lines to the first section `name`, leaving out those whose key (or, for lines without
    /// one, whose text) is there already; a missing section is created at the end of the file.
    pub fn add(&mut self, name: &str, new: &[String]) {
        let Some((s, e)) = self.section(name) else {
            self.lines.push(String::new());
            self.lines.push(format!("[{name}]"));
            self.lines.extend(new.iter().cloned());
            return;
        };
        let id = |l: &str| key(l).unwrap_or_else(|| l.trim().to_lowercase());
        let have: std::collections::HashSet<String> =
            self.lines[s..e].iter().map(|l| id(l)).collect();
        let add: Vec<String> = new
            .iter()
            .filter(|l| !have.contains(&id(l)))
            .cloned()
            .collect();
        self.lines.splice(e..e, add);
    }

    /// Replaces the line with key `k` in the first section `name`; there must be exactly one.
    pub fn replace(&mut self, name: &str, k: &str, new: &str) -> Result<()> {
        let (s, e) = self.require(name)?;
        let k = k.to_lowercase();
        let hits: Vec<usize> = (s..e)
            .filter(|&i| key(&self.lines[i]).as_deref() == Some(k.as_str()))
            .collect();
        if hits.len() != 1 {
            bail!("[{name}] {k}: {} matches", hits.len());
        }
        self.lines[hits[0]] = new.to_string();
        Ok(())
    }

    /// The value of key `k` in the first section `name` (text after the first '=', trimmed).
    pub fn get(&self, name: &str, k: &str) -> Option<String> {
        let (s, e) = self.section(name)?;
        let k = k.to_lowercase();
        self.lines[s..e]
            .iter()
            .find(|l| key(l).as_deref() == Some(k.as_str()))
            .map(|l| l.split_once('=').unwrap().1.trim().to_string())
    }

    /// Inserts a line right after the header of the first section `name`.
    pub fn insert_after_header(&mut self, name: &str, line: &str) -> Result<()> {
        let (s, _) = self.require(name)?;
        self.lines.insert(s, line.to_string());
        Ok(())
    }

    /// Copies the lines of `src` whose key `want` accepts into the same sections here (after the first
    /// occurrence of each), unless the key is in one of that section's occurrences already. Returns
    /// (section, keys added) per section, in the order of `src`.
    pub fn merge_keys(
        &mut self,
        src: &Text,
        want: impl Fn(&str) -> bool,
    ) -> Result<Vec<(String, Vec<String>)>> {
        let ssec = src.sections();
        let dsec = self.sections();
        let mut names: Vec<String> = Vec::new();
        for (n, _, _) in &ssec {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        let mut inserts: Vec<(usize, String, Vec<String>)> = Vec::new();
        for name in names {
            let body = |t: &Text, secs: &[(String, usize, usize)]| -> Vec<String> {
                secs.iter()
                    .filter(|(n, _, _)| *n == name)
                    .flat_map(|&(_, h, e)| t.lines[h + 1..e].to_vec())
                    .collect()
            };
            let wanted: Vec<String> = body(src, &ssec)
                .into_iter()
                .filter(|l| key(l).is_some_and(|k| want(&k)))
                .collect();
            if wanted.is_empty() {
                continue;
            }
            let Some(&(_, _, first_end)) = dsec.iter().find(|(n, _, _)| *n == name) else {
                bail!("section [{name}] missing");
            };
            let have: std::collections::HashSet<String> =
                body(self, &dsec).iter().filter_map(|l| key(l)).collect();
            let add: Vec<String> = wanted
                .into_iter()
                .filter(|l| !have.contains(&key(l).unwrap()))
                .collect();
            if !add.is_empty() {
                inserts.push((first_end, name, add));
            }
        }
        let report = inserts
            .iter()
            .map(|(_, n, a)| (n.clone(), a.iter().filter_map(|l| key(l)).collect()))
            .collect();
        inserts.sort_by_key(|i| std::cmp::Reverse(i.0));
        for (pos, _, add) in inserts {
            self.lines.splice(pos..pos, add);
        }
        Ok(report)
    }
}

/// An INI file as WINNT.SIF is read and merged: sections and keys in order, names compared without
/// case, comments and blank lines dropped.
#[derive(Debug, Clone, Default)]
pub struct Ini {
    /// (title as first written, [(key as first written, value)])
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut cur: Option<usize> = None;
        for l in text.replace('\r', "").split('\n') {
            let t = l.trim_end();
            if let Some(name) =
                header(t).filter(|_| t.trim_start().starts_with('[') && t.ends_with(']'))
            {
                cur = Some(ini.section_index(name));
            } else if let Some(c) = cur
                && !l.trim().is_empty()
                && !l.trim_start().starts_with(';')
            {
                let (k, v) = l.split_once('=').unwrap_or((l, ""));
                ini.set_at(c, k.trim(), v.trim());
            }
        }
        ini
    }

    fn section_index(&mut self, name: &str) -> usize {
        match self
            .sections
            .iter()
            .position(|(t, _)| t.eq_ignore_ascii_case(name))
        {
            Some(i) => i,
            None => {
                self.sections.push((name.to_string(), Vec::new()));
                self.sections.len() - 1
            }
        }
    }

    fn set_at(&mut self, s: usize, k: &str, v: &str) {
        let keys = &mut self.sections[s].1;
        // A key set again keeps its place but takes the new spelling and value.
        match keys.iter_mut().find(|(kk, _)| kk.eq_ignore_ascii_case(k)) {
            Some(kv) => *kv = (k.to_string(), v.to_string()),
            None => keys.push((k.to_string(), v.to_string())),
        }
    }

    pub fn get(&self, section: &str, k: &str) -> Option<&str> {
        let (_, keys) = self
            .sections
            .iter()
            .find(|(t, _)| t.eq_ignore_ascii_case(section))?;
        keys.iter()
            .find(|(kk, _)| kk.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, section: &str, k: &str, v: &str) {
        let s = self.section_index(section);
        self.set_at(s, k, v);
    }

    /// Lines: a blank line and the header before each section, `key=value` for each key.
    pub fn to_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (title, keys) in &self.sections {
            out.push(String::new());
            out.push(format!("[{title}]"));
            out.extend(keys.iter().map(|(k, v)| format!("{k}={v}")));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_bytes() {
        let b = b"[A]\r\nx = 1\r\n; c\xb2\xe2\r\n\r\n[B]\r\ny=2".to_vec();
        assert_eq!(Text::parse(&b).unwrap().to_bytes().unwrap(), b);
        let u: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain("[A]\r\nä=1\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(Text::parse(&u).unwrap().to_bytes().unwrap(), u);
    }

    #[test]
    fn add_replace_and_new_sections() {
        let mut t = Text::parse(b"[A]\r\nx = 1\r\n\r\n[B]\r\n").unwrap();
        t.add("a", &["X=9".into(), "y = 2".into()]);
        t.replace("A", "x", "x = 3").unwrap();
        t.add("C", &["z".into()]);
        assert_eq!(t.lines.join("|"), "[A]|x = 3|y = 2||[B]|||[C]|z");
        assert!(t.replace("B", "x", "").is_err());
    }

    #[test]
    fn ini_merge() {
        let mut a = Ini::parse("; c\r\n[Data]\r\nA=1\r\n[unattended]\r\nB = 2\r\n");
        let b = Ini::parse("[DATA]\na=3\nC=4\n");
        for (t, keys) in &b.sections {
            for (k, v) in keys {
                a.set(t, k, v);
            }
        }
        assert_eq!(a.to_lines().join("|"), "|[Data]|a=3|C=4||[unattended]|B=2");
    }
}
