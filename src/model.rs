use std::path::{Path, PathBuf};

/// Live status of a worktree, filled in by the background status pass.
#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeStatus {
    /// Status pass hasn't completed yet.
    Pending,
    /// `git status` failed for this worktree (e.g. directory is gone).
    Unavailable(String),
    Clean {
        ahead: u32,
        behind: u32,
    },
    Dirty {
        staged: u32,
        unstaged: u32,
        untracked: u32,
        ahead: u32,
        behind: u32,
    },
}

#[derive(Clone, Debug)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub is_main: bool,
    pub status: WorktreeStatus,
}

/// Parses `git worktree list --porcelain`. Lenient: unknown lines are ignored.
pub fn parse_worktree_porcelain(input: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: rest.into(),
                head: None,
                branch: None,
                is_main: false,
                status: WorktreeStatus::Pending,
            });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            if let Some(e) = current.as_mut() {
                e.head = Some(rest.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("branch ") {
            if let Some(e) = current.as_mut() {
                e.branch = Some(rest.trim_start_matches("refs/heads/").to_string());
            }
        }
        // "detached", "bare", "locked …", "prunable …" are recognized by git
        // but carry nothing v1 records.
    }
    if let Some(e) = current.take() {
        entries.push(e);
    }
    // git always lists the main worktree first.
    if let Some(first) = entries.first_mut() {
        first.is_main = true;
    }
    entries
}

/// Parses `git status --porcelain=v2 --branch`. Lenient: unknown lines are
/// ignored; missing branch.ab means 0/0 ahead/behind.
pub fn parse_status_porcelain_v2(input: &str) -> WorktreeStatus {
    let (mut staged, mut unstaged, mut untracked) = (0u32, 0u32, 0u32);
    let (mut ahead, mut behind) = (0u32, 0u32);
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut parts = rest.split(' ');
            if let Some(a) = parts.next() {
                ahead = a.trim_start_matches('+').parse().unwrap_or(0);
            }
            if let Some(b) = parts.next() {
                behind = b.trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if line.starts_with("? ") {
            untracked += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") || line.starts_with("u ") {
            let mut fields = line.split(' ');
            let _ordinal = fields.next();
            if let Some(xy) = fields.next() {
                let x = xy.chars().next().unwrap_or('.');
                let y = xy.chars().nth(1).unwrap_or('.');
                if x != '.' && x != '!' {
                    staged += 1;
                }
                if y != '.' && y != '!' {
                    unstaged += 1;
                }
            }
        }
        // remaining "# …" headers and unrecognized rows are ignored
    }
    if staged + unstaged + untracked == 0 {
        WorktreeStatus::Clean { ahead, behind }
    } else {
        WorktreeStatus::Dirty {
            staged,
            unstaged,
            untracked,
            ahead,
            behind,
        }
    }
}

pub fn sanitize_branch(branch: &str) -> String {
    branch.replace('/', "-")
}

/// Default location for a new worktree: a sibling directory of the repo root,
/// e.g. `/Users/greg/git/myrepo` + branch `feature/x` →
/// `/Users/greg/git/myrepo-worktrees/feature-x`.
pub fn default_worktree_path(repo_root: &Path, branch: &str) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    repo_root
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{repo_name}-worktrees"))
        .join(sanitize_branch(branch))
}

/// Case-insensitive substring match on branch name and path.
pub fn matches_filter(entry: &WorktreeEntry, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let f = filter.to_lowercase();
    entry
        .branch
        .as_deref()
        .is_some_and(|b| b.to_lowercase().contains(&f))
        || entry.path.to_string_lossy().to_lowercase().contains(&f)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "worktree /Users/greg/git/myrepo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /Users/greg/git/myrepo-worktrees/feature-x\nHEAD def456\nbranch refs/heads/feature-x\n\nworktree /Users/greg/git/myrepo-worktrees/det\nHEAD 789abc\ndetached\n";

    fn entry(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: path.into(),
            head: None,
            branch: branch.map(String::from),
            is_main: false,
            status: WorktreeStatus::Pending,
        }
    }

    #[test]
    fn parses_main_linked_and_detached() {
        let entries = parse_worktree_porcelain(LIST);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_main);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[0].head.as_deref(), Some("abc123"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature-x"));
        assert!(!entries[1].is_main);
        assert_eq!(entries[2].branch, None); // detached
        assert!(entries
            .iter()
            .all(|e| matches!(e.status, WorktreeStatus::Pending)));
    }

    #[test]
    fn ignores_unknown_lines_and_trailing_blank() {
        let entries = parse_worktree_porcelain(&format!(
            "{LIST}\nworktree /x\nlocked reason\nprunable gitdir is gone\n"
        ));
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn empty_input_yields_no_entries() {
        assert!(parse_worktree_porcelain("").is_empty());
    }

    #[test]
    fn sanitizes_slashes_in_branch_names() {
        assert_eq!(sanitize_branch("feature/x-y"), "feature-x-y");
    }

    #[test]
    fn derives_default_path_next_to_repo() {
        let p = default_worktree_path(Path::new("/Users/greg/git/myrepo"), "feature/x");
        assert_eq!(
            p,
            PathBuf::from("/Users/greg/git/myrepo-worktrees/feature-x")
        );
    }

    #[test]
    fn filter_matches_branch_and_path_case_insensitively() {
        let e = entry("/a/b/Feature-X", Some("feat"));
        assert!(matches_filter(&e, "FEAT"));
        assert!(matches_filter(&e, "/a/b/"));
        assert!(!matches_filter(&e, "zzz"));
        assert!(matches_filter(&e, ""));
    }

    const STATUS: &str = "# branch.oid abc (initial)\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +1 -2\n1 .M N... 100100 100100 100100 a1b2c3 a1b2c3 f.txt\n1 M. N... 100100 100100 100100 a1b2c3 a1b2c3 staged.txt\n2 R. N... 100100 100100 100100 a1b2c3 a1b2c3 R100 renamed\ntree new/old\n? untracked.txt\n";

    #[test]
    fn status_parses_counts_and_divergence() {
        assert_eq!(
            parse_status_porcelain_v2(STATUS),
            WorktreeStatus::Dirty {
                staged: 2,    // M. + R.
                unstaged: 1,  // .M
                untracked: 1, // ?
                ahead: 1,
                behind: 2,
            }
        );
    }

    #[test]
    fn status_clean_when_no_changes() {
        let input = "# branch.head main\n# branch.ab +1 -2\n";
        assert_eq!(
            parse_status_porcelain_v2(input),
            WorktreeStatus::Clean {
                ahead: 1,
                behind: 2
            }
        );
    }

    #[test]
    fn status_defaults_and_detached() {
        assert_eq!(
            parse_status_porcelain_v2(""),
            WorktreeStatus::Clean {
                ahead: 0,
                behind: 0
            }
        );
        assert_eq!(
            parse_status_porcelain_v2("# branch.head (detached)\n"),
            WorktreeStatus::Clean {
                ahead: 0,
                behind: 0
            }
        );
    }
}
