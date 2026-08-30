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
    /// The full `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
    /// Byte-exact hunk text (header + lines), consumed verbatim by
    /// `git apply --cached` in Phase 1b hunk staging.
    pub raw: String,
}

#[derive(Clone, Debug, Default)]
pub struct UnifiedDiff {
    /// Everything before the first hunk: `diff --git`, index, `---/+++`,
    /// rename/mode lines.
    pub header: String,
    pub hunks: Vec<DiffHunk>,
    pub binary: bool,
}

/// Parses a single-file `git diff -U3 --no-color` output.
pub fn parse_unified_diff(input: &str) -> UnifiedDiff {
    let mut diff = UnifiedDiff::default();
    let mut header = String::new();
    let mut cur: Option<DiffHunk> = None;
    let mut cur_raw = String::new();
    for line in input.split_inclusive('\n') {
        let line = line.strip_suffix('\n').unwrap_or(line);
        if diff.binary {
            continue;
        }
        if line.starts_with("@@") {
            if let Some(mut h) = cur.take() {
                h.raw = std::mem::take(&mut cur_raw);
                diff.hunks.push(h);
            }
            cur = Some(DiffHunk {
                header: line.to_string(),
                lines: Vec::new(),
                raw: String::new(),
            });
            cur_raw.push_str(line);
            cur_raw.push('\n');
            continue;
        }
        if cur.is_none() {
            if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                diff.binary = true;
            }
            header.push_str(line);
            header.push('\n');
            continue;
        }
        cur_raw.push_str(line);
        cur_raw.push('\n');
        let hunk = cur.as_mut().expect("checked Some above");
        match line.chars().next() {
            Some('+') => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Add,
                content: line[1..].to_string(),
                no_newline: false,
            }),
            Some('-') => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Del,
                content: line[1..].to_string(),
                no_newline: false,
            }),
            Some('\\') => {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
            }
            _ => hunk.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                content: line.strip_prefix(' ').unwrap_or(line).to_string(),
                no_newline: false,
            }),
        }
    }
    if let Some(mut h) = cur.take() {
        h.raw = cur_raw;
        diff.hunks.push(h);
    }
    diff.header = header;
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/f.txt b/f.txt\nindex a1b2c3..d4e5f6 100644\n--- a/f.txt\n+++ b/f.txt\n@@ -1,2 +1,3 @@\n one\n+two\n three\n@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n";

    #[test]
    fn parses_hunks_lines_and_no_newline_marker() {
        let d = parse_unified_diff(PATCH);
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
        let d = parse_unified_diff(PATCH);
        let raw = &d.hunks[0].raw;
        assert!(raw.starts_with("@@ -1,2 +1,3 @@\n one\n+two\n three\n"));
        assert_eq!(
            &d.hunks[1].raw,
            "@@ -10,2 +11,2 @@\n four\n-five\n+six\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn detects_binary_and_mode_only_changes() {
        let d = parse_unified_diff("diff --git a/img.png b/img.png\nindex a..b 100644\nBinary files a/img.png and b/img.png differ\n");
        assert!(d.binary);
        assert!(d.hunks.is_empty());
        let d = parse_unified_diff("diff --git a/s.sh b/s.sh\nold mode 100644\nnew mode 100755\n");
        assert!(!d.binary);
        assert!(d.hunks.is_empty());
    }

    #[test]
    fn empty_input_is_empty_diff() {
        let d = parse_unified_diff("");
        assert!(d.hunks.is_empty());
        assert!(d.header.is_empty());
    }
}
