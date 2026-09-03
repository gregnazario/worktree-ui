use crate::engine::{self, Result};
use std::io::Read as _;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line content without the leading ` `/`+`/`-` marker.
    pub content: String,
    /// Preceded by a `\ No newline at end of file` marker.
    pub no_newline: bool,
}

#[derive(Clone, Debug)]
pub struct DiffHunk {
    /// The full `@@ -a,b +c,d @@ context` line (lossy-decoded for display).
    pub header: String,
    pub lines: Vec<DiffLine>,
    /// Byte-exact hunk text (header + lines), consumed verbatim by
    /// `git apply --cached` in Phase 1b hunk staging. Bytes, because diff
    /// content and header paths may not be valid UTF-8.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct UnifiedDiff {
    /// Everything before the first hunk: `diff --git`, index, `---/+++`,
    /// rename/mode lines (lossy-decoded for display).
    pub header: String,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

/// Parses a single-file `git diff -U3 --no-color` output. Display strings
/// are lossy-decoded, but every hunk's `raw` preserves the exact bytes.
pub fn parse_unified_diff(input: &[u8]) -> UnifiedDiff {
    let mut diff = UnifiedDiff::default();
    let mut header: Vec<u8> = Vec::new();
    let mut cur: Option<DiffHunk> = None;
    let mut cur_raw: Vec<u8> = Vec::new();
    for line in input.split_inclusive(|b| *b == b'\n') {
        if diff.binary {
            continue;
        }
        if line.starts_with(b"@@") {
            if let Some(mut h) = cur.take() {
                h.raw = std::mem::take(&mut cur_raw);
                diff.hunks.push(h);
            }
            cur = Some(DiffHunk {
                header: String::from_utf8_lossy(line).into_owned(),
                lines: Vec::new(),
                raw: Vec::new(),
            });
            cur_raw.extend_from_slice(line);
            continue;
        }
        if cur.is_none() {
            if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
                diff.binary = true;
            }
            header.extend_from_slice(line);
            continue;
        }
        cur_raw.extend_from_slice(line);
        let stripped = line.strip_suffix(b"\n").unwrap_or(line);
        let hunk = cur.as_mut().expect("checked Some above");
        match stripped.first() {
            Some(b'+') => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Add,
                content: String::from_utf8_lossy(&stripped[1..]).into_owned(),
                no_newline: false,
            }),
            Some(b'-') => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Del,
                content: String::from_utf8_lossy(&stripped[1..]).into_owned(),
                no_newline: false,
            }),
            Some(b'\\') => {
                // `\ No newline at end of file` — annotate the previous line.
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
            }
            first => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                content: match first {
                    Some(b' ') => String::from_utf8_lossy(&stripped[1..]).into_owned(),
                    _ => String::from_utf8_lossy(stripped).into_owned(),
                },
                no_newline: false,
            }),
        }
    }
    if let Some(mut h) = cur.take() {
        h.raw = cur_raw;
        diff.hunks.push(h);
    }
    diff.header = String::from_utf8_lossy(&header).into_owned();
    diff
}

/// Single known path → `:(literal)` pathspec (never glob-interpreted).
fn literal(rel_path: &str) -> String {
    format!(":(literal){rel_path}")
}

pub fn diff_unstaged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff> {
    // NOTE: `--no-optional-locks` is a GLOBAL git option — it must appear
    // before the `diff` subcommand or git exits 129.
    let out = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "diff",
            "--no-color",
            "--no-ext-diff",
            "-U3",
            "--",
            &literal(rel_path),
        ],
    )?;
    Ok(parse_unified_diff(&out))
}

pub fn diff_staged(worktree: &Path, rel_path: &str) -> Result<UnifiedDiff> {
    // NOTE: `--no-optional-locks` is a GLOBAL git option — it must appear
    // before the `diff` subcommand or git exits 129.
    let out = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "-U3",
            "--",
            &literal(rel_path),
        ],
    )?;
    Ok(parse_unified_diff(&out))
}

pub const PREVIEW_MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub enum Preview {
    Text { content: String, truncated: bool },
    Binary,
    Directory,
    Missing,
}

/// Working-tree content for untracked and conflicted files (which `git
/// diff` doesn't express), bounded to `PREVIEW_MAX_BYTES` with a NUL sniff
/// over the first 8 KiB for binary detection.
pub fn read_preview(worktree: &Path, rel_path: &str) -> Preview {
    let full = worktree.join(rel_path);
    match std::fs::metadata(&full) {
        Ok(m) if m.is_dir() => return Preview::Directory,
        // Non-regular files (FIFOs, sockets, devices): opening a FIFO read
        // -only blocks until a writer appears, hanging the background
        // thread forever — never read them.
        Ok(m) if !m.is_file() => return Preview::Binary,
        Err(_) => return Preview::Missing,
        Ok(_) => {}
    }
    let Ok(file) = std::fs::File::open(&full) else {
        return Preview::Missing;
    };
    let mut bytes = Vec::new();
    let mut limited = file.take((PREVIEW_MAX_BYTES + 1) as u64);
    if limited.read_to_end(&mut bytes).is_err() {
        return Preview::Missing;
    }
    let sniff = bytes.len().min(8192);
    if bytes[..sniff].contains(&0) {
        return Preview::Binary;
    }
    let truncated = bytes.len() > PREVIEW_MAX_BYTES;
    if truncated {
        bytes.truncate(PREVIEW_MAX_BYTES);
    }
    Preview::Text {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/f.txt b/f.txt\nindex a1b2c3..d4e5f6 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n one\n+two\n three\n@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n";

    #[test]
    fn parses_hunks_lines_and_no_newline_marker() {
        let d = parse_unified_diff(PATCH.as_bytes());
        assert_eq!(d.hunks.len(), 2);
        assert!(!d.binary);
        assert!(d.header.contains("diff --git a/f.txt"));
        let h0 = &d.hunks[0];
        assert!(h0.header.starts_with("@@ -1,2 +1,3 @@"));
        assert_eq!(h0.lines.len(), 3);
        assert_eq!(h0.lines[0].kind, DiffLineKind::Context);
        assert_eq!(h0.lines[0].content, "one");
        assert_eq!(h0.lines[1].kind, DiffLineKind::Add);
        assert_eq!(h0.lines[1].content, "two");
        let h1 = &d.hunks[1];
        assert_eq!(h1.lines.len(), 3);
        assert_eq!(h1.lines[2].kind, DiffLineKind::Add);
        assert!(h1.lines[2].no_newline, "marker annotates the last + line");
        assert!(!h1.lines.iter().any(|l| l.content.starts_with('\\')));
    }

    #[test]
    fn raw_is_byte_exact_for_hunk_staging() {
        let d = parse_unified_diff(PATCH.as_bytes());
        let raw = &d.hunks[0].raw;
        assert!(raw.starts_with(b"@@ -1,2 +1,3 @@\n one\n+two\n three\n"));
        assert_eq!(
            d.hunks[1].raw,
            b"@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn raw_preserves_non_utf8_bytes_exactly() {
        // 0xFF is not valid UTF-8: display text must lossy-decode, but the
        // hunk raw — the Phase 1b `git apply --cached` payload — must keep
        // the original bytes.
        let mut patch = b"diff --git a/b b/b\n@@ -1 +1 @@\n-old\xff\n".to_vec();
        patch.extend_from_slice(b"+new\xfe\n");
        let d = parse_unified_diff(&patch);
        assert_eq!(d.hunks.len(), 1);
        assert_eq!(d.hunks[0].raw, patch[b"diff --git a/b b/b\n".len()..]);
        assert!(d.hunks[0].lines[0].content.contains('\u{FFFD}'));
    }

    #[test]
    fn detects_binary_and_mode_only_changes() {
        let d = parse_unified_diff("diff --git a/img.png b/img.png\nindex a..b 100644\nBinary files a/img.png and b/img.png differ\n".as_bytes());
        assert!(d.binary);
        assert!(d.hunks.is_empty());
        let d = parse_unified_diff(
            "diff --git a/s.sh b/s.sh\nold mode 100644\nnew mode 100755\n".as_bytes(),
        );
        assert!(!d.binary);
        assert!(d.hunks.is_empty());
    }

    #[test]
    fn empty_input_is_empty_diff() {
        let d = parse_unified_diff(b"");
        assert!(d.hunks.is_empty());
        assert!(d.header.is_empty());
    }
}
