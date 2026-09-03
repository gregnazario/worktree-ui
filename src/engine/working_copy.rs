use crate::engine;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchInfo {
    /// `# branch.head` value: branch name or `(detached)`.
    pub head: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    /// Repo-root-relative path in git's form (forward slashes).
    pub path: String,
    /// Rename/copy source (`2` records only).
    pub orig_path: Option<String>,
    /// X: index status letter (`.` when unchanged).
    pub index_status: char,
    /// Y: worktree status letter.
    pub wt_status: char,
    /// Unmerged code (`UU`, `AA`, `DU`, …) from `u` records.
    pub conflict: Option<String>,
    pub untracked: bool,
    /// numstat (+added, −deleted); `None` = binary or not applicable.
    pub staged_lines: Option<(u64, u64)>,
    pub unstaged_lines: Option<(u64, u64)>,
    /// The raw filename was not valid UTF-8, so the lossy path string here
    /// is mangled: `:(literal)` pathspecs can't match it, and the store
    /// refuses mutations on this entry rather than failing opaquely.
    pub unsupported: bool,
}

impl FileEntry {
    /// Collapsed untracked directories are listed as `dir/` by git.
    pub fn is_dir(&self) -> bool {
        self.path.ends_with('/')
    }
}

#[derive(Clone, Debug)]
pub struct WorkingCopy {
    pub branch: BranchInfo,
    pub entries: Vec<FileEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Conflicts,
    Staged,
    Unstaged,
    Untracked,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Conflicts => "Conflicts",
            Group::Staged => "Staged",
            Group::Unstaged => "Unstaged",
            Group::Untracked => "Untracked",
        }
    }
}

/// Parses `git status --porcelain=v2 -z --branch`. With `-z`, records are
/// NUL-terminated and header lines are LF-terminated inside one chunk; a
/// rename record's orig path rides in the NEXT NUL chunk. Lenient: unknown
/// chunks are ignored.
pub fn parse_status_z(input: &str) -> WorkingCopy {
    let mut wc = WorkingCopy {
        branch: BranchInfo::default(),
        entries: Vec::new(),
    };
    let mut expect_orig_path = false;
    for chunk in input.split('\0') {
        if expect_orig_path {
            // Consume unconditionally: the orig path could theoretically
            // start with characters that look like a record.
            if let Some(last) = wc.entries.last_mut() {
                last.orig_path = Some(chunk.to_string());
            }
            expect_orig_path = false;
            continue;
        }
        if chunk.is_empty() {
            continue;
        }
        if chunk.starts_with('#') {
            for line in chunk.split('\n') {
                if let Some(rest) = line.strip_prefix("# branch.head ") {
                    wc.branch.head = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
                    wc.branch.upstream = Some(rest.trim().to_string());
                } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
                    for part in rest.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            wc.branch.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            wc.branch.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
            }
            continue;
        }
        if let Some(path) = chunk.strip_prefix("? ") {
            wc.entries.push(FileEntry {
                path: path.to_string(),
                orig_path: None,
                index_status: '?',
                wt_status: '?',
                conflict: None,
                untracked: true,
                staged_lines: None,
                unstaged_lines: None,
                unsupported: false,
            });
            continue;
        }
        if chunk.starts_with("1 ") {
            let f: Vec<&str> = chunk.splitn(9, ' ').collect();
            if f.len() == 9 {
                let (x, y) = xy(f[1]);
                wc.entries.push(FileEntry {
                    path: f[8].to_string(),
                    orig_path: None,
                    index_status: x,
                    wt_status: y,
                    conflict: None,
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                    unsupported: false,
                });
            }
            continue;
        }
        if chunk.starts_with("2 ") {
            let f: Vec<&str> = chunk.splitn(10, ' ').collect();
            if f.len() == 10 {
                let (x, y) = xy(f[1]);
                let is_rename = f[8].starts_with('R') || f[8].starts_with('C');
                wc.entries.push(FileEntry {
                    path: f[9].to_string(),
                    orig_path: None,
                    index_status: x,
                    wt_status: y,
                    conflict: None,
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                    unsupported: false,
                });
                if is_rename {
                    expect_orig_path = true;
                }
            }
            continue;
        }
        if chunk.starts_with("u ") {
            let f: Vec<&str> = chunk.splitn(11, ' ').collect();
            if f.len() == 11 {
                wc.entries.push(FileEntry {
                    path: f[10].to_string(),
                    orig_path: None,
                    index_status: 'U',
                    wt_status: 'U',
                    conflict: Some(f[1].to_string()),
                    untracked: false,
                    staged_lines: None,
                    unstaged_lines: None,
                    unsupported: false,
                });
            }
            continue;
        }
        // "!" ignored records and anything unknown: ignore.
    }
    wc
}

fn xy(field: &str) -> (char, char) {
    let mut chars = field.chars();
    (chars.next().unwrap_or('.'), chars.next().unwrap_or('.'))
}

/// Parses `git diff --numstat -z`: records are `added\tdeleted\tpath` NUL-
/// terminated; rename records are followed by their orig path in the next
/// NUL chunk (no tabs — skipped here). `-` counts mean binary.
pub fn parse_numstat_z(input: &str) -> Vec<(String, Option<(u64, u64)>)> {
    let mut out = Vec::new();
    for chunk in input.split('\0') {
        if chunk.is_empty() {
            continue;
        }
        let mut parts = chunk.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue; // rename orig-path chunk has no tabs
        };
        let counts = if a == "-" || d == "-" {
            None
        } else {
            Some((a.parse().unwrap_or(0), d.parse().unwrap_or(0)))
        };
        out.push((path.to_string(), counts));
    }
    out
}

/// Full working-copy snapshot: status v2 `-z` + numstat for both diff
/// surfaces. Read-only (`--no-optional-locks`, which git only accepts as a
/// global option BEFORE the subcommand).
pub fn status(worktree: &Path) -> engine::Result<WorkingCopy> {
    let raw_bytes = engine::run_bytes(
        worktree,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            // Explicit so the user's `status.showUntrackedFiles` config
            // can't silently empty the Untracked group.
            "--untracked-files=normal",
        ],
    )?;
    // Strict UTF-8 first: a filename that legitimately contains U+FFFD
    // survives strict decoding (and its pathspec works), so it is NOT
    // unsupported. Only when the bytes aren't valid UTF-8 do we fall back
    // to lossy decoding and flag the mangled entries.
    let (raw, strict) = match String::from_utf8(raw_bytes) {
        Ok(s) => (s, true),
        Err(e) => (String::from_utf8_lossy(e.as_bytes()).into_owned(), false),
    };
    let mut wc = parse_status_z(&raw);
    if !strict {
        for entry in wc.entries.iter_mut() {
            if entry.path.contains('\u{FFFD}')
                || entry
                    .orig_path
                    .as_deref()
                    .is_some_and(|p| p.contains('\u{FFFD}'))
            {
                entry.unsupported = true;
            }
        }
    }
    // Path → entry index, so numstat records merge in O(1) instead of a
    // linear scan per record (O(n·m) on large trees).
    let entry_by_path: std::collections::HashMap<String, usize> = wc
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.path.clone(), i))
        .collect();
    for (args, key) in [
        (
            vec!["--no-optional-locks", "diff", "--numstat", "-z"],
            0usize,
        ),
        (
            vec!["--no-optional-locks", "diff", "--cached", "--numstat", "-z"],
            1,
        ),
    ] {
        let raw = engine::run(worktree, &args)?;
        for (path, counts) in parse_numstat_z(&raw) {
            if let Some(&i) = entry_by_path.get(path.as_str()) {
                match key {
                    0 => wc.entries[i].unstaged_lines = counts,
                    _ => wc.entries[i].staged_lines = counts,
                }
            }
        }
    }
    Ok(wc)
}

/// Display order with stable indices into `wc.entries`. A file changed in
/// both index and worktree appears in both Staged and Unstaged.
pub fn group_rows(wc: &WorkingCopy) -> Vec<(Group, usize)> {
    let mut rows = Vec::new();
    for (i, e) in wc.entries.iter().enumerate() {
        if e.conflict.is_some() {
            rows.push((Group::Conflicts, i));
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if !e.untracked && e.conflict.is_none() && e.index_status != '.' {
            rows.push((Group::Staged, i));
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if !e.untracked && e.conflict.is_none() && e.wt_status != '.' {
            rows.push((Group::Unstaged, i));
        }
    }
    for (i, e) in wc.entries.iter().enumerate() {
        if e.untracked {
            rows.push((Group::Untracked, i));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Status v2 -z: header chunk (LF-joined # lines), a `1` record staged,
    /// a `2` rename record whose orig path rides in the NEXT NUL chunk, an
    /// unmerged `u` record, an untracked `?` record, and an unstaged-only
    /// `1` record so every group has a row.
    const Z: &str = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n\u{0}1 M. N... 100100 100100 100100 a1 a1 staged.txt\u{0}2 R. N... 100100 100100 100100 a1 a1 R100 new/name.txt\u{0}old/name.txt\u{0}u UU N... 100 100 100 100 h1 h2 h3 conflicted.txt\u{0}? untracked dir/file with spaces.txt\u{0}1 .M N... 100100 100100 100100 a1 a1 modified.txt\u{0}";

    #[test]
    fn parses_headers_records_renames_conflicts_untracked() {
        let wc = parse_status_z(Z);
        assert_eq!(wc.branch.head, "main");
        assert_eq!(wc.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!((wc.branch.ahead, wc.branch.behind), (2, 1));
        assert_eq!(wc.entries.len(), 5);

        assert_eq!(wc.entries[0].path, "staged.txt");
        assert_eq!(wc.entries[0].index_status, 'M');
        assert_eq!(wc.entries[0].wt_status, '.');

        assert_eq!(wc.entries[1].path, "new/name.txt");
        assert_eq!(wc.entries[1].orig_path.as_deref(), Some("old/name.txt"));

        assert_eq!(wc.entries[2].conflict.as_deref(), Some("UU"));
        assert_eq!(wc.entries[2].path, "conflicted.txt");

        let un = &wc.entries[3];
        assert!(un.untracked);
        assert_eq!(un.path, "untracked dir/file with spaces.txt");
    }

    #[test]
    fn paths_with_spaces_survive_splitn() {
        let wc = parse_status_z("1 .M N... 1 1 1 a b my file.txt\u{0}");
        assert_eq!(wc.entries[0].path, "my file.txt");
    }

    #[test]
    fn detached_head_and_empty_input() {
        let wc = parse_status_z("# branch.head (detached)\n\u{0}");
        assert_eq!(wc.branch.head, "(detached)");
        assert!(wc.entries.is_empty());
        let wc = parse_status_z("");
        assert_eq!(wc.branch.head, "");
        assert!(wc.entries.is_empty());
    }

    #[test]
    fn group_rows_orders_conflicts_staged_unstaged_untracked() {
        let wc = parse_status_z(Z);
        let rows = group_rows(&wc);
        let groups: Vec<Group> = rows.iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            vec![
                Group::Conflicts,
                Group::Staged,
                Group::Staged,
                Group::Unstaged,
                Group::Untracked
            ]
        );
        // same file staged+unstaged appears in both groups:
        let both = parse_status_z("1 MM N... 1 1 1 a b both.txt\u{0}");
        let groups: Vec<Group> = group_rows(&both).iter().map(|(g, _)| *g).collect();
        assert_eq!(groups, vec![Group::Staged, Group::Unstaged]);
    }

    #[test]
    fn untracked_directory_row_is_detected() {
        let wc = parse_status_z("? vendor/\u{0}");
        assert!(wc.entries[0].is_dir());
    }
}
