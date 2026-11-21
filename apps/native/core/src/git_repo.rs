//! Local Git probes and explicitly scoped worktree operations.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow, ensure};
use icu_normalizer::ComposingNormalizer;
use icu_properties::CodePointMapData;
use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Branch(String),
    Detached(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub revision: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub main: bool,
    pub bare: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBranch {
    pub name: String,
    pub current: bool,
    pub commit_timestamp: i64,
    pub checked_out_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBranches {
    pub default: Option<String>,
    pub branches: Vec<LocalBranch>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeCounts {
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusDetail {
    pub repository_root: PathBuf,
    pub current_branch: Option<String>,
    pub detached_head: Option<String>,
    pub branches: Vec<LocalBranch>,
    pub dirty: bool,
    pub dirty_file_count: usize,
    pub staged_count: usize,
    pub additions: u64,
    pub deletions: u64,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub has_remote: bool,
    pub can_push: bool,
    pub worktrees: Vec<GitWorktreeDetail>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktreeDetail {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub revision: Option<String>,
    pub current: bool,
    pub main: bool,
    pub dirty: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Debug)]
pub struct GitActionError(pub &'static str);

impl std::fmt::Display for GitActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for GitActionError {}

fn require_repository(cwd: &Path) -> Result<Repository> {
    Repository::discover(cwd)?.ok_or_else(|| GitActionError("GIT_NOT_REPOSITORY").into())
}

pub fn git_status_detail(cwd: &Path) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let (current_branch, detached_head) = match repo.head()? {
        Head::Branch(name) => (Some(name), None),
        Head::Detached(revision) => (None, Some(revision)),
    };
    let root = repo.root();
    let (dirty_file_count, staged_count, _) = detailed_changes(root)?;
    let head = repo.resolve_commit("HEAD");
    let base = match &head {
        Ok(revision) => revision.clone(),
        Err(_) => String::from_utf8(run(root, &["hash-object", "-t", "tree", "--stdin"])?)?
            .trim()
            .to_owned(),
    };
    let diff = run(
        root,
        &[
            "diff",
            "--numstat",
            "--no-ext-diff",
            "--no-textconv",
            &base,
            "--",
        ],
    )?;
    let (mut additions, mut deletions) = (0, 0);
    for row in diff.split(|byte| *byte == b'\n') {
        let mut fields = row.splitn(3, |byte| *byte == b'\t');
        let count = |field: Option<&[u8]>| {
            field
                .and_then(|field| std::str::from_utf8(field).ok())
                .and_then(|field| field.parse::<u64>().ok())
                .unwrap_or_default()
        };
        additions += count(fields.next());
        deletions += count(fields.next());
    }
    let tracking = upstream_ref(&repo, current_branch.as_deref())?;
    let tracking_revision = tracking
        .as_ref()
        .and_then(|(reference, _, _)| repo.resolve_commit(reference).ok());
    let (ahead, behind) = if let (Ok(head), Some(revision)) = (&head, &tracking_revision) {
        let range = format!("{head}...{revision}");
        let counts = run(root, &["rev-list", "--left-right", "--count", &range])?;
        let mut counts = std::str::from_utf8(&counts)?.split_whitespace();
        (
            counts.next().context("missing Git ahead count")?.parse()?,
            counts.next().context("missing Git behind count")?.parse()?,
        )
    } else {
        (0, 0)
    };
    let has_remote = !run(root, &["remote"])?.is_empty();
    let can_push = has_remote
        && head.is_ok()
        && current_branch.is_some()
        && (tracking_revision.is_none() || ahead > 0);
    let entries = repo.worktrees()?;
    let branches = repo
        .local_branches_from(&entries, current_branch.as_deref())?
        .branches;
    let worktrees = entries
        .into_iter()
        .map(|entry| {
            let current = entry.path == root;
            let dirty = if current {
                dirty_file_count > 0
            } else {
                !entry.bare && is_dirty(&entry.path).unwrap_or(false)
            };
            GitWorktreeDetail {
                path: entry.path,
                branch: entry.branch,
                revision: entry.revision,
                current,
                main: entry.main,
                dirty,
                locked: entry.locked || entry.bare,
                prunable: entry.prunable,
            }
        })
        .collect();
    Ok(GitStatusDetail {
        repository_root: root.to_owned(),
        current_branch,
        detached_head,
        branches,
        dirty: dirty_file_count != 0,
        dirty_file_count,
        staged_count,
        additions,
        deletions,
        upstream: tracking.map(|(reference, _, _)| reference),
        ahead,
        behind,
        has_remote,
        can_push,
        worktrees,
    })
}

fn detailed_changes(root: &Path) -> Result<(usize, usize, bool)> {
    let output = run(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
    )?;
    let (mut files, mut staged, mut unmerged) = (0, 0, false);
    let mut records = output.split(|byte| *byte == 0);
    while let Some(record) = records.next().filter(|record| record.len() >= 3) {
        let xy = &record[..2];
        files += 1;
        unmerged |= matches!(xy, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU");
        staged += usize::from(xy[0] != b' ' && xy != b"??");
        if xy.iter().any(|flag| matches!(flag, b'R' | b'C')) {
            records.next();
        }
    }
    Ok((files, staged, unmerged))
}

// Git reports both the local tracking ref and its actual destination on the remote.
fn upstream_ref(
    repo: &Repository,
    branch: Option<&str>,
) -> Result<Option<(String, String, String)>> {
    let Some(branch) = branch else {
        return Ok(None);
    };
    let reference = format!("refs/heads/{branch}");
    let output = run(
        repo.root(),
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)",
            &reference,
        ],
    )?;
    for row in std::str::from_utf8(&output)?.lines() {
        let mut fields = row.split('\0');
        if let (Some(name), Some(tracking), Some(remote), Some(destination)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
            && name == reference
            && !tracking.is_empty()
        {
            return Ok(Some((tracking.into(), remote.into(), destination.into())));
        }
    }
    Ok(None)
}

fn checked_branch<'a>(repo: &Repository, input: &'a str) -> Result<&'a str> {
    let name = match manual_branch_name(input) {
        Ok(name) => name,
        Err(ManualBranchError::Empty) => return Err(GitActionError("GIT_BRANCH_REQUIRED").into()),
        Err(ManualBranchError::LeadingDash) => {
            return Err(GitActionError("GIT_INVALID_BRANCH").into());
        }
    };
    ensure_branch_format(repo, name)?;
    Ok(name)
}

fn ensure_branch_format(repo: &Repository, name: &str) -> Result<()> {
    let checked = git(repo.root(), &["check-ref-format", "--branch", name])?;
    if !checked.status.success() || line(&checked.stdout) != name.as_bytes() {
        return Err(GitActionError("GIT_INVALID_BRANCH").into());
    }
    Ok(())
}

pub fn git_checkout(cwd: &Path, branch: &str) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let branch = checked_branch(&repo, branch)?;
    if repo.head()? != Head::Branch(branch.into()) && !repo.local_branch_exists(branch)? {
        return Err(GitActionError("GIT_BRANCH_MISSING").into());
    }
    git_write(repo.root(), &["checkout", "--no-guess", branch, "--"])?;
    git_status_detail(repo.root())
}

pub fn git_create_branch(
    cwd: &Path,
    name: &str,
    start_point: Option<&str>,
) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let name = checked_branch(&repo, name)?;
    if repo.local_branch_exists(name)? {
        return Err(GitActionError("GIT_BRANCH_EXISTS").into());
    }
    if let Some(start) = start_point.map(trim_js).filter(|value| !value.is_empty()) {
        let revision = repo
            .resolve_commit(start)
            .map_err(|_| GitActionError("GIT_INVALID_START_POINT"))?;
        git_write(repo.root(), &["checkout", "-b", name, &revision, "--"])?;
    } else {
        git_write(repo.root(), &["checkout", "-b", name, "--"])?;
    }
    git_status_detail(repo.root())
}

pub fn git_commit(cwd: &Path, message: &str, stage_all: bool) -> Result<GitStatusDetail> {
    let message = trim_js(message);
    if message.is_empty() {
        return Err(GitActionError("GIT_COMMIT_MESSAGE_REQUIRED").into());
    }
    let repo = require_repository(cwd)?;
    let (files, staged, unmerged) = detailed_changes(repo.root())?;
    if unmerged {
        return Err(GitActionError("GIT_UNMERGED").into());
    }
    if (stage_all && files == 0) || (!stage_all && staged == 0) {
        return Err(GitActionError("GIT_NOTHING_TO_COMMIT").into());
    }
    if stage_all {
        git_write(repo.root(), &["add", "-A"])?;
    }
    git_write(repo.root(), &["commit", "-m", message])?;
    git_status_detail(repo.root())
}

pub fn git_push(cwd: &Path) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let Head::Branch(branch) = repo.head()? else {
        return Err(GitActionError("GIT_DETACHED_HEAD").into());
    };
    if !repo.local_branch_exists(&branch)? {
        return Err(GitActionError("GIT_NOTHING_TO_PUSH").into());
    }
    let tracking = upstream_ref(&repo, Some(&branch))?;
    let reference = format!("refs/heads/{branch}");
    if let Some((_, remote, destination)) = tracking {
        if remote.is_empty() || !destination.starts_with("refs/heads/") {
            return Err(GitActionError("GIT_UPSTREAM_UNAVAILABLE").into());
        }
        let refspec = format!("{reference}:{destination}");
        git_write(repo.root(), &["push", "--", &remote, &refspec])?;
    } else {
        let output = run(repo.root(), &["remote"])?;
        let mut remotes = std::str::from_utf8(&output)?.lines();
        let first = remotes.next().ok_or(GitActionError("GIT_NO_REMOTE"))?;
        let remote = if first == "origin" || remotes.any(|name| name == "origin") {
            "origin"
        } else {
            first
        };
        let refspec = format!("{reference}:{reference}");
        git_write(
            repo.root(),
            &["push", "--set-upstream", "--", remote, &refspec],
        )?;
    }
    git_status_detail(repo.root())
}

fn git_write(root: &Path, args: &[&str]) -> Result<()> {
    let output = git(root, args)?;
    if output.status.success() {
        return Ok(());
    }
    let error = git_failure(&output);
    let message = error.to_string();
    let code = if message.contains("would be overwritten by checkout") {
        "GIT_CHECKOUT_CONFLICT"
    } else if message.contains("already checked out")
        || message.contains("already used by worktree")
    {
        "GIT_BRANCH_IN_WORKTREE"
    } else if message.contains("non-fast-forward") || message.contains("(fetch first)") {
        "GIT_PUSH_REJECTED"
    } else if message.contains("has no upstream branch")
        || message.contains("upstream branch of your current branch")
    {
        "GIT_UPSTREAM_UNAVAILABLE"
    } else if message.contains("resolve your current index") || message.contains("unmerged files") {
        "GIT_UNMERGED"
    } else if message.contains("nothing to commit")
        || message.contains("no changes added to commit")
    {
        "GIT_NOTHING_TO_COMMIT"
    } else {
        return Err(error);
    };
    Err(error.context(GitActionError(code)))
}

pub fn git_worktree_create(
    cwd: &Path,
    branch: &str,
    path: Option<&Path>,
) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let name = checked_branch(&repo, branch)?;
    let entries = repo.worktrees()?;
    let main = entries.first().context("missing main Git worktree")?;
    let path = match path {
        Some(path) if path.is_absolute() => path.to_owned(),
        Some(path) => cwd.join(path),
        None => main.path.with_file_name(format!(
            "{}-{}",
            main.path.file_name().unwrap_or_default().to_string_lossy(),
            name.replace('/', "-")
        )),
    };
    let branch = if repo.local_branch_exists(name)? {
        if entries
            .iter()
            .any(|entry| entry.branch.as_deref() == Some(name))
        {
            return Err(GitActionError("GIT_BRANCH_IN_WORKTREE").into());
        }
        WorktreeBranch::Existing(name)
    } else {
        WorktreeBranch::New {
            name,
            base_ref: "HEAD",
        }
    };
    checkout_scope(&repo, &path)?.create(&path, branch)?;
    git_status_detail(repo.root())
}

pub fn git_worktree_remove(cwd: &Path, worktree_path: &Path) -> Result<GitStatusDetail> {
    let repo = require_repository(cwd)?;
    let path = if worktree_path.is_absolute() {
        worktree_path.to_owned()
    } else {
        cwd.join(worktree_path)
    };
    let target = canonicalize_missing(&path)?;
    let entries = repo.worktrees()?;
    let entry = entries
        .iter()
        .find(|entry| entry.path == target)
        .ok_or(GitActionError("GIT_WORKTREE_UNKNOWN"))?;
    if entry.main || entry.bare || entry.path == repo.root() || entry.locked {
        return Err(GitActionError("GIT_WORKTREE_PROTECTED").into());
    }
    if checkout_scope(&repo, &path)?.remove(&path)? == Removal::RetainedDirty {
        return Err(GitActionError("GIT_WORKTREE_DIRTY").into());
    }
    git_status_detail(repo.root())
}

fn checkout_scope(repo: &Repository, path: &Path) -> Result<WorktreeScope> {
    // The Git menu supplies the exact target. Scope checkout and rollback to its
    // parent, including when the app's Worktree directory is on another volume.
    let root = canonicalize_missing(path.parent().context("worktree needs a parent directory")?)?;
    Ok(WorktreeScope {
        root,
        repository: repo.clone(),
    })
}

impl ChangeCounts {
    pub fn total(self) -> usize {
        self.staged + self.unstaged + self.untracked
    }
}

/// Read the symbolic HEAD for a working directory without invoking Git or
/// touching the index. Linked worktrees store their own HEAD behind a gitfile.
pub fn current_branch(cwd: &Path) -> Option<String> {
    worktree_head(cwd)?
        .trim_end()
        .strip_prefix("ref: refs/heads/")
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
}

fn worktree_head(cwd: &Path) -> Option<String> {
    let cwd = fs::canonicalize(cwd).ok()?;
    if !cwd.is_dir() {
        return None;
    }
    let ceilings = std::env::var_os("GIT_CEILING_DIRECTORIES");
    for dir in cwd.ancestors() {
        if dir != cwd
            && ceilings
                .as_ref()
                .is_some_and(|paths| std::env::split_paths(paths).any(|path| path == dir))
        {
            return None;
        }
        let marker = dir.join(".git");
        let Ok(metadata) = fs::metadata(&marker) else {
            continue;
        };
        let git_dir = if metadata.is_dir() {
            marker
        } else {
            let gitfile = read_small_git_file(&marker)?;
            dir.join(gitfile.strip_prefix("gitdir: ")?.trim_end())
        };
        return read_small_git_file(&git_dir.join("HEAD"));
    }
    None
}

/// Query the worktree without writing the index or waiting on a long scan.
pub fn is_dirty(cwd: &Path) -> Option<bool> {
    use std::{
        io::Read,
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    if worktree_head(cwd).is_none() {
        return Some(false);
    }
    let start = Instant::now();
    let mut child = git_command(cwd)
        .args([
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take().expect("piped Git status");
    let output = thread::Builder::new()
        .name("git-status-output".into())
        .spawn(move || {
            let mut first = [0];
            let dirty = stdout.read(&mut first).ok()? != 0;
            std::io::copy(&mut stdout, &mut std::io::sink()).ok()?;
            Some(dirty)
        });
    if let Ok(output) = output {
        while start.elapsed() < Duration::from_millis(40) {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return status
                        .success()
                        .then(|| output.join().ok().flatten())
                        .flatten();
                }
                Ok(None) => thread::sleep(Duration::from_millis(1)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

fn read_small_git_file(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8192 {
        return None;
    }
    fs::read_to_string(path).ok()
}

impl Repository {
    /// Probes the repository for a directory, including a subdirectory of a linked worktree.
    pub fn discover(path: &Path) -> Result<Option<Self>> {
        let cwd = fs::canonicalize(path).with_context(|| format!("resolve {}", path.display()))?;
        let inside = git(&cwd, &["rev-parse", "--is-inside-work-tree"])?;
        if !inside.status.success()
            && String::from_utf8_lossy(&inside.stderr).contains("not a git repository")
        {
            return Ok(None);
        }
        if stdout(inside)? != b"true\n" {
            return Ok(None);
        }
        let resolve = |flag| -> Result<PathBuf> {
            let output = run(&cwd, &["rev-parse", "--path-format=absolute", flag])?;
            Ok(fs::canonicalize(git_path(line(&output)))?)
        };
        Ok(Some(Self {
            root: resolve("--show-toplevel")?,
            git_dir: resolve("--absolute-git-dir")?,
            common_dir: resolve("--git-common-dir")?,
        }))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn same_repository(&self, other: &Self) -> bool {
        self.common_dir == other.common_dir
    }

    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }

    pub fn head(&self) -> Result<Head> {
        match symbolic_ref(&self.root, "HEAD")? {
            Some(branch) => Ok(Head::Branch(
                branch.strip_prefix("refs/heads/").unwrap_or(&branch).into(),
            )),
            None => Ok(Head::Detached(self.resolve_commit("HEAD")?)),
        }
    }

    pub fn resolve_commit(&self, reference: &str) -> Result<String> {
        let commit = format!("{reference}^{{commit}}");
        let output = run(
            &self.root,
            &["rev-parse", "--verify", "--end-of-options", &commit],
        )?;
        Ok(std::str::from_utf8(line(&output))?.into())
    }

    pub fn changes(&self) -> Result<ChangeCounts> {
        let output = run(
            &self.root,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?;
        Ok(parse_changes(&output))
    }

    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let output = run(&self.root, &["worktree", "list", "--porcelain", "-z"])?;
        let mut entries = parse_worktrees(&output)?;
        for entry in &mut entries {
            if let Ok(path) = canonicalize_missing(&entry.path) {
                entry.path = path;
            }
        }
        Ok(entries)
    }

    /// Returns the innermost worktree containing an existing path.
    pub fn worktree_for_path(&self, path: &Path) -> Result<Option<Worktree>> {
        let path = fs::canonicalize(path)?;
        Ok(self
            .worktrees()?
            .into_iter()
            .filter(|entry| !entry.bare && path.starts_with(&entry.path))
            .max_by_key(|entry| entry.path.components().count()))
    }

    pub fn local_branches(&self) -> Result<LocalBranches> {
        let current = match self.head()? {
            Head::Branch(branch) => Some(branch),
            Head::Detached(_) => None,
        };
        self.local_branches_from(&self.worktrees()?, current.as_deref())
    }

    fn local_branches_from(
        &self,
        worktrees: &[Worktree],
        current: Option<&str>,
    ) -> Result<LocalBranches> {
        let output = run(
            &self.root,
            &[
                "for-each-ref",
                "--format=%(refname)%00%(committerdate:unix)",
                "refs/heads",
            ],
        )?;
        let origin_head = symbolic_ref(&self.root, "refs/remotes/origin/HEAD")?;
        branch_summaries(&output, current, origin_head.as_deref(), worktrees)
    }

    pub fn check_fork_branch(&self, branch: &str) -> Result<(), ForkBranchError> {
        // A successful check must echo the exact input. Git can expand @{-1}.
        ensure_branch_format(self, branch).map_err(|_| ForkBranchError::Invalid)?;
        match self.local_branch_exists(branch) {
            Ok(true) => Err(ForkBranchError::LocalConflict),
            Ok(false) => Ok(()),
            Err(error) => Err(ForkBranchError::Git(error)),
        }
    }

    fn local_branch_exists(&self, branch: &str) -> Result<bool> {
        let reference = format!("refs/heads/{branch}");
        let result = git(&self.root, &["show-ref", "--verify", "--quiet", &reference])?;
        match result.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_failure(&result)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayNameError {
    Empty,
    TooLong,
    Duplicate,
}

pub fn validate_display_name<'a>(
    input: &'a str,
    active_sibling_names: &[&str],
) -> Result<&'a str, DisplayNameError> {
    let name = trim_js(input);
    if name.is_empty() {
        return Err(DisplayNameError::Empty);
    }
    if name.chars().count() > 64 {
        return Err(DisplayNameError::TooLong);
    }
    if active_sibling_names.contains(&name) {
        return Err(DisplayNameError::Duplicate);
    }
    Ok(name)
}

pub fn create_git_branch_name(display_name: &str, session_id: &str) -> String {
    let normalized = ComposingNormalizer::new_nfkc()
        .normalize(trim_js(display_name))
        .to_lowercase();
    let categories = CodePointMapData::<GeneralCategory>::new();
    let mut slug = String::new();
    for ch in normalized.chars() {
        let category = categories.get(ch);
        if GeneralCategoryGroup::Letter.contains(category)
            || GeneralCategoryGroup::Number.contains(category)
        {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(48).collect();
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "branch" } else { slug };

    let normalized_id = trim_js(session_id).to_ascii_lowercase();
    let identity = match normalized_id.as_bytes().split_first_chunk::<8>() {
        Some((prefix, rest))
            if prefix.iter().all(u8::is_ascii_hexdigit) && rest.starts_with(b"-") =>
        {
            normalized_id[..8].to_owned()
        }
        _ => format!("{:x}", Sha256::digest(session_id.as_bytes()))[..8].to_owned(),
    };
    format!("matou/{slug}-{identity}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualBranchError {
    Empty,
    LeadingDash,
}

/// The manual Git entry point trims its argument and lets Git validate the remaining format.
pub fn manual_branch_name(input: &str) -> Result<&str, ManualBranchError> {
    let branch = trim_js(input);
    if branch.is_empty() {
        return Err(ManualBranchError::Empty);
    }
    if branch.starts_with('-') {
        return Err(ManualBranchError::LeadingDash);
    }
    Ok(branch)
}

fn trim_js(input: &str) -> &str {
    input.trim_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}')
}

#[derive(Debug)]
pub enum ForkBranchError {
    Invalid,
    LocalConflict,
    Reserved,
    Git(anyhow::Error),
}

impl std::fmt::Display for ForkBranchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => f.write_str("invalid branch name"),
            Self::LocalConflict => f.write_str("local branch already exists"),
            Self::Reserved => f.write_str("branch reserved by another fork"),
            Self::Git(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ForkBranchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

/// The caller supplies durable reservation rows. An absent fork intent is different
/// from an in-progress intent when the associated worktree has been removed.
pub struct BranchReservation<'a> {
    pub repository_root: &'a Path,
    pub branch: &'a str,
    pub worktree_removed: bool,
    pub fork: Option<ForkClaim<'a>>,
}

#[derive(Clone, Copy)]
pub struct ForkClaim<'a> {
    pub operation_id: &'a str,
    pub submission_key: &'a str,
    pub finished: bool,
}

pub fn check_branch_reservations(
    repository_root: &Path,
    branch: &str,
    operation_id: &str,
    submission_key: &str,
    reservations: &[BranchReservation<'_>],
) -> Result<(), ForkBranchError> {
    for row in reservations {
        if row.repository_root.as_os_str() != repository_root.as_os_str() || row.branch != branch {
            continue;
        }
        let same_owner = match row.fork {
            Some(fork) => {
                fork.operation_id == operation_id && fork.submission_key == submission_key
            }
            None => operation_id.is_empty() && submission_key.is_empty(),
        };
        if !same_owner && (!row.worktree_removed || row.fork.is_some_and(|fork| !fork.finished)) {
            return Err(ForkBranchError::Reserved);
        }
    }
    Ok(())
}

pub enum WorktreeBranch<'a> {
    New { name: &'a str, base_ref: &'a str },
    Existing(&'a str),
}

/// Scopes checkout writes to a directory and a verified repository identity.
/// Targets must be separate from all registered worktrees.
pub struct WorktreeScope {
    root: PathBuf,
    repository: Repository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    RetainedDirty,
}

impl WorktreeScope {
    /// Authorizes fork operations in a caller-owned directory containing the
    /// repository, its Git metadata, and all requested targets.
    pub fn new(isolated_root: &Path, repository: &Repository) -> Result<Self> {
        let root = fs::canonicalize(isolated_root)?;
        ensure!(
            root.is_dir() && root.parent().is_some(),
            "invalid worktree isolation directory"
        );
        for path in [
            &repository.root,
            &repository.common_dir,
            &repository.git_dir,
        ] {
            ensure!(
                strictly_within(&root, path),
                "repository outside worktree isolation directory"
            );
        }
        Ok(Self {
            root,
            repository: repository.clone(),
        })
    }

    pub fn create(&self, path: &Path, branch: WorktreeBranch<'_>) -> Result<Worktree> {
        self.verify_repository()?;
        let path = self.target_path(path)?;
        let entries = self.repository.worktrees()?;
        ensure!(
            !entries.iter().any(|entry| overlaps(&entry.path, &path)),
            "worktree path conflicts with a registered worktree: {}",
            path.display()
        );
        let (name, revision, create_branch) = match branch {
            WorktreeBranch::New { name, base_ref } => {
                self.repository.check_fork_branch(name)?;
                let base_ref = trim_js(base_ref);
                let base_ref = if base_ref.is_empty() {
                    "HEAD"
                } else {
                    base_ref
                };
                (name, self.repository.resolve_commit(base_ref)?, true)
            }
            WorktreeBranch::Existing(name) => {
                let name = manual_branch_name(name).map_err(|_| anyhow!("invalid branch name"))?;
                ensure!(
                    self.repository.local_branch_exists(name)?,
                    "branch missing: {name}"
                );
                ensure!(
                    !entries
                        .iter()
                        .any(|entry| entry.branch.as_deref() == Some(name)),
                    "branch already checked out: {name}"
                );
                let reference = format!("refs/heads/{name}");
                (name, self.repository.resolve_commit(&reference)?, false)
            }
        };
        // An atomic directory reservation ensures we never clean up a pre-existing path.
        fs::create_dir(&path).with_context(|| format!("reserve {}", path.display()))?;
        let reference = format!("refs/heads/{name}");
        let mut owns_branch = false;
        let result = (|| {
            if create_branch {
                self.run(&["update-ref", "--no-deref", &reference, &revision, ""])?;
                owns_branch = true;
            }
            self.run(&[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--no-guess-remote"),
                OsStr::new("--"),
                path.as_os_str(),
                OsStr::new(name),
            ])?;
            let checkout = self.linked_repository(&path)?;
            ensure!(
                self.registered_path(&checkout)? == path
                    && checkout.head()? == Head::Branch(name.into())
                    && checkout.resolve_commit("HEAD")? == revision,
                "worktree did not check out {name} at {revision}"
            );
            let entries = self.repository.worktrees()?;
            require_separate_target(&entries, &path)?;
            entries
                .into_iter()
                .find(|entry| entry.path == path)
                .context("new checkout is absent from Git worktree metadata")
        })();
        match result {
            Ok(worktree) => Ok(worktree),
            Err(error) => match self.rollback(&path, name, &reference, &revision, owns_branch) {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("worktree rollback failed: {cleanup:#}")))
                }
            },
        }
    }

    pub fn remove(&self, path: &Path) -> Result<Removal> {
        self.verify_repository()?;
        let path = self.target_path(path)?;
        self.require_registered_target(&path)?;
        if path_is_missing(&path)? {
            self.remove_registered(&path)?;
            return Ok(Removal::Removed);
        }
        let checkout = self.linked_repository(&path)?;
        ensure!(
            self.registered_path(&checkout)? == path,
            "checkout belongs to a different worktree"
        );
        if checkout.changes()?.total() != 0 {
            return Ok(Removal::RetainedDirty);
        }
        self.remove_registered(&path)?;
        Ok(Removal::Removed)
    }

    /// Re-registers an existing moved checkout after checking its repository, HEAD
    /// and original registration. It never creates a checkout or overwrites a directory.
    pub fn repair_moved(&self, original: &Path, path: &Path, expected: &Head) -> Result<()> {
        let (path, needs_repair) = self.inspect_moved(original, path, expected)?;
        if !needs_repair {
            return Ok(());
        }
        self.run(&[
            OsStr::new("worktree"),
            OsStr::new("repair"),
            path.as_os_str(),
        ])?;
        self.require_registered_target(&path)
    }

    pub fn needs_moved_repair(
        &self,
        original: &Path,
        path: &Path,
        expected: &Head,
    ) -> Result<bool> {
        self.inspect_moved(original, path, expected)
            .map(|(_, needs_repair)| needs_repair)
    }

    fn inspect_moved(
        &self,
        original: &Path,
        path: &Path,
        expected: &Head,
    ) -> Result<(PathBuf, bool)> {
        self.verify_repository()?;
        let original = self.target_path(original)?;
        let path = self.target_path(path)?;
        let checkout = self.linked_repository(&path)?;
        ensure!(
            &checkout.head()? == expected,
            "worktree HEAD does not match"
        );
        let old_path = self.registered_path(&checkout)?;
        ensure!(
            original == old_path,
            "candidate belongs to another worktree"
        );
        let entries = self.repository.worktrees()?;
        if entries
            .iter()
            .any(|entry| entry.path == path && !entry.main)
        {
            ensure!(original == path, "candidate is another registered worktree");
            require_separate_target(&entries, &path)?;
            return Ok((path, false));
        }
        ensure!(
            !entries.iter().any(|entry| overlaps(&entry.path, &path)),
            "worktree path conflicts with a registered worktree"
        );

        // The checkout's .git file can also have been copied. Refuse to redirect
        // the original registration while its checkout is still present.
        ensure!(
            path_is_missing(&old_path)?,
            "original worktree path still exists"
        );
        Ok((path, true))
    }

    fn verify_repository(&self) -> Result<()> {
        ensure!(
            Repository::discover(&self.repository.root)?.as_ref() == Some(&self.repository),
            "scoped repository identity changed"
        );
        Ok(())
    }

    fn target_path(&self, path: &Path) -> Result<PathBuf> {
        let name = path.file_name().context("worktree path needs a name")?;
        let parent = path.parent().context("worktree path needs a parent")?;
        // Resolve the nearest existing ancestor, including for moved worktrees whose
        // old parent directory is gone. Git requires an existing parent for create.
        let path = canonicalize_missing(parent)?.join(name);
        ensure!(
            strictly_within(&self.root, &path) && !overlaps(&self.repository.common_dir, &path),
            "worktree path outside isolation directory or overlaps Git metadata: {}",
            path.display()
        );
        match fs::symlink_metadata(&path) {
            Ok(info) => ensure!(
                !info.is_symlink(),
                "worktree path is a symlink: {}",
                path.display()
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(path)
    }

    fn linked_repository(&self, path: &Path) -> Result<Repository> {
        let checkout = Repository::discover(path)?.context("path is not a Git worktree")?;
        ensure!(
            checkout.root() == path
                && checkout.same_repository(&self.repository)
                && strictly_within(
                    &self.repository.common_dir.join("worktrees"),
                    &checkout.git_dir
                ),
            "path is not a linked checkout of the scoped repository: {}",
            path.display()
        );
        Ok(checkout)
    }

    fn require_registered_target(&self, path: &Path) -> Result<()> {
        let entries = self.repository.worktrees()?;
        ensure!(
            entries
                .iter()
                .any(|entry| entry.path == path && !entry.main && !entry.bare),
            "path is not a registered linked worktree: {}",
            path.display()
        );
        require_separate_target(&entries, path)
    }

    fn registered_path(&self, checkout: &Repository) -> Result<PathBuf> {
        let registration = fs::read(checkout.git_dir.join("gitdir"))?;
        let marker = checkout.git_dir.join(git_path(line(&registration)));
        let path = marker
            .parent()
            .context("missing registered worktree path")?;
        self.target_path(path)
    }

    fn remove_registered(&self, path: &Path) -> Result<()> {
        self.run(&[
            OsStr::new("worktree"),
            OsStr::new("remove"),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        Ok(())
    }

    fn rollback(
        &self,
        path: &Path,
        branch: &str,
        reference: &str,
        revision: &str,
        owns_branch: bool,
    ) -> Result<()> {
        self.verify_repository()?;
        ensure!(self.target_path(path)? == path, "worktree path changed");
        let entries = self.repository.worktrees()?;
        if let Some(entry) = entries.iter().find(|entry| entry.path == path) {
            ensure!(
                !entry.main && entry.branch.as_deref() == Some(branch),
                "worktree registration changed during creation"
            );
            require_separate_target(&entries, path)?;
            let checkout = self.linked_repository(path)?;
            ensure!(
                self.registered_path(&checkout)? == path,
                "checkout belongs to a different worktree"
            );
            self.remove_registered(path)?;
        } else if !path_is_missing(path)? {
            // Git cleans its checkout on failure. Only remove our empty reservation.
            fs::remove_dir(path)?;
        }
        if owns_branch {
            ensure!(
                !self
                    .repository
                    .worktrees()?
                    .iter()
                    .any(|entry| entry.branch.as_deref() == Some(branch)),
                "branch is now checked out"
            );
            self.run(&["update-ref", "--no-deref", "-d", reference, revision])?;
        }
        Ok(())
    }

    fn run(&self, args: &[impl AsRef<OsStr>]) -> Result<Vec<u8>> {
        let mut command = git_command(&self.repository.root);
        // Worktree plumbing must not execute interactive checkout or ref hooks.
        let no_hooks = if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        };
        let result = command
            .args(["-c", no_hooks])
            .args(args)
            .output()
            .context("run Git")?;
        stdout(result)
    }
}

fn strictly_within(root: &Path, path: &Path) -> bool {
    path != root && path.starts_with(root)
}

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn require_separate_target(entries: &[Worktree], path: &Path) -> Result<()> {
    ensure!(
        !entries
            .iter()
            .any(|entry| entry.path != path && overlaps(&entry.path, path)),
        "path overlaps another registered worktree: {}",
        path.display()
    );
    Ok(())
}

fn path_is_missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn canonicalize_missing(path: &Path) -> Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == ErrorKind::NotFound && path_is_missing(path)? => {
            let name = path.file_name().context("missing path has no name")?;
            let parent = path.parent().context("missing path has no parent")?;
            Ok(canonicalize_missing(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn parse_worktrees(output: &[u8]) -> Result<Vec<Worktree>> {
    let mut result = Vec::new();
    let mut fields = output.split(|byte| *byte == 0);
    while let Some(header) = fields.next().filter(|header| !header.is_empty()) {
        let path = header
            .strip_prefix(b"worktree ")
            .context("invalid Git worktree header")?;
        let mut entry = Worktree {
            path: git_path(path),
            revision: None,
            branch: None,
            detached: false,
            main: result.is_empty(),
            bare: false,
            locked: false,
            prunable: false,
        };
        for field in fields.by_ref().take_while(|field| !field.is_empty()) {
            let mut parts = field.splitn(2, |byte| *byte == b' ');
            let key = parts.next().unwrap_or_default();
            let value = parts.next().unwrap_or_default();
            match key {
                b"HEAD" => entry.revision = Some(std::str::from_utf8(value)?.into()),
                b"branch" => {
                    let name = value.strip_prefix(b"refs/heads/").unwrap_or(value);
                    entry.branch = Some(std::str::from_utf8(name)?.into());
                }
                b"detached" => entry.detached = true,
                b"bare" => entry.bare = true,
                b"locked" => entry.locked = true,
                b"prunable" => entry.prunable = true,
                _ => {}
            }
        }
        result.push(entry);
    }
    Ok(result)
}

fn parse_changes(output: &[u8]) -> ChangeCounts {
    let mut result = ChangeCounts::default();
    for line in output.split(|byte| *byte == b'\n') {
        if line.len() < 3 {
            continue;
        }
        if line.starts_with(b"??") {
            result.untracked += 1;
        } else {
            result.staged += usize::from(line[0] != b' ');
            result.unstaged += usize::from(line[1] != b' ');
        }
    }
    result
}

fn branch_summaries(
    output: &[u8],
    current: Option<&str>,
    origin_head: Option<&str>,
    worktrees: &[Worktree],
) -> Result<LocalBranches> {
    let mut branches = Vec::new();
    for row in std::str::from_utf8(output)?
        .lines()
        .filter(|row| !row.is_empty())
    {
        let (reference, timestamp) = row.split_once('\0').context("invalid Git branch row")?;
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        let checked_out_path = worktrees
            .iter()
            .rev()
            .find(|entry| entry.branch.as_deref() == Some(name))
            .map(|entry| entry.path.clone());
        branches.push(LocalBranch {
            name: name.into(),
            current: current == Some(name),
            commit_timestamp: timestamp.parse().unwrap_or_default(),
            checked_out_path,
        });
    }
    let default = origin_head
        .and_then(|reference| reference.strip_prefix("refs/remotes/origin/"))
        .into_iter()
        .chain(["main", "master"])
        .find(|name| branches.iter().any(|branch| branch.name == *name))
        .map(str::to_owned)
        .or_else(|| branches.first().map(|branch| branch.name.clone()));
    branches.sort_by(|left, right| {
        let priority = |branch: &LocalBranch| {
            if Some(&branch.name) == default.as_ref() {
                0
            } else if branch.current {
                1
            } else {
                2
            }
        };
        priority(left)
            .cmp(&priority(right))
            .then_with(|| right.commit_timestamp.cmp(&left.commit_timestamp))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(LocalBranches { default, branches })
}

fn symbolic_ref(cwd: &Path, reference: &str) -> Result<Option<String>> {
    let output = git(cwd, &["symbolic-ref", "--quiet", reference])?;
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Ok(Some(std::str::from_utf8(line(&stdout(output)?))?.into()))
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    // Inherited checkout variables can redirect an otherwise scoped command
    // to the caller's Git directory, index or object store.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_SHALLOW_FILE",
        "GIT_GRAFT_FILE",
        "GIT_IMPLICIT_WORK_TREE",
    ] {
        command.env_remove(key);
    }
    command
}

fn git(cwd: &Path, args: &[&str]) -> Result<Output> {
    git_command(cwd).args(args).output().context("run Git")
}

fn run(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    stdout(git(cwd, args)?)
}

fn stdout(output: Output) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(git_failure(&output))
}

fn git_failure(output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = stderr.trim();
    if !message.is_empty() {
        return anyhow!("{message}");
    }
    if !stdout.trim().is_empty() {
        return anyhow!("{}", stdout.trim());
    }
    anyhow!("Git operation failed ({})", output.status)
}

fn line(output: &[u8]) -> &[u8] {
    output.strip_suffix(b"\n").unwrap_or(output)
}

#[cfg(unix)]
fn git_path(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec()).into()
}

#[cfg(not(unix))]
fn git_path(bytes: &[u8]) -> PathBuf {
    OsString::from(String::from_utf8_lossy(bytes).into_owned()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn cases(operation: &str) -> impl Iterator<Item = Value> + '_ {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../../tooling/native-parity/fixtures/git-branch-policy-reference.json"
        ))
        .unwrap();
        fixture["cases"]
            .as_array()
            .unwrap()
            .clone()
            .into_iter()
            .filter(move |case| case["input"]["operation"] == operation)
    }

    #[test]
    fn hud_git_state_tracks_untracked_and_staged_changes() -> Result<()> {
        let root = tempfile::tempdir()?;
        assert_eq!(current_branch(root.path()), None);
        assert_eq!(is_dirty(root.path()), Some(false));
        run(root.path(), &["init", "-b", "hud-fixture"])?;
        assert_eq!(current_branch(root.path()).as_deref(), Some("hud-fixture"));
        assert_eq!(is_dirty(root.path()), Some(false));
        fs::write(root.path().join("tracked.txt"), "fixture")?;
        assert_eq!(is_dirty(root.path()), Some(true));
        run(root.path(), &["add", "tracked.txt"])?;
        assert_eq!(is_dirty(root.path()), Some(true));
        Ok(())
    }

    #[test]
    fn generated_git_refs_match_reference() {
        let mut checked = 0;
        for case in cases("generate-ref") {
            let input = &case["input"];
            let display_name = input["displayName"].as_str().unwrap();
            let actual: Vec<_> = input["sessionIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| create_git_branch_name(display_name, id.as_str().unwrap()))
                .collect();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                case["expected"]["branches"],
                "{}",
                case["id"],
            );
            checked += 1;
        }
        assert_eq!(checked, 8);
    }

    #[test]
    fn display_names_match_reference() {
        let mut checked = 0;
        for case in cases("validate-display-name") {
            let input = &case["input"];
            let name = input["name"].as_str().unwrap();
            let siblings: Vec<_> = input["activeSiblingNames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect();
            let actual = validate_display_name(name, &siblings);
            let expected = &case["expected"]["validation"];
            let expectation = match expected["code"].as_str() {
                Some("EMPTY") => Err(DisplayNameError::Empty),
                Some("TOO_LONG") => Err(DisplayNameError::TooLong),
                Some("DUPLICATE") => Err(DisplayNameError::Duplicate),
                None => Ok(expected["displayName"].as_str().unwrap()),
                _ => panic!("unexpected error for {}", case["id"]),
            };
            assert_eq!(actual, expectation, "{}", case["id"]);
            checked += 1;
        }
        assert_eq!(checked, 7);
    }

    #[test]
    fn manual_branch_validation_preserves_explicit_names() {
        assert_eq!(manual_branch_name("  feature/next  "), Ok("feature/next"));
        assert_eq!(manual_branch_name(" \t "), Err(ManualBranchError::Empty));
        assert_eq!(
            manual_branch_name("  -sample "),
            Err(ManualBranchError::LeadingDash)
        );
    }

    #[test]
    fn durable_reservations_match_reference() {
        let mut checked = 0;
        for case in cases("check-fork-reservation") {
            let input = &case["input"];
            let worktree = &input["worktrees"][0];
            let intent = &input["intents"][0];
            let repository_root = Path::new(worktree[1].as_str().unwrap());
            let branch = input["branch"].as_str().unwrap();
            let reservation = BranchReservation {
                repository_root,
                branch: worktree[2].as_str().unwrap(),
                worktree_removed: worktree[3] == "removed",
                fork: Some(ForkClaim {
                    operation_id: intent[2].as_str().unwrap(),
                    submission_key: intent[3].as_str().unwrap(),
                    finished: intent[1] == "succeeded" || intent[1] == "failed",
                }),
            };
            let same_claim = reservation.fork.unwrap();
            let reservations = [reservation];
            let actual = check_branch_reservations(
                repository_root,
                branch,
                input["operationId"].as_str().unwrap(),
                input["submissionKey"].as_str().unwrap(),
                &reservations,
            );
            if case["expected"]["result"] == "available" {
                assert!(actual.is_ok(), "{}", case["id"]);
            } else {
                assert!(
                    matches!(actual, Err(ForkBranchError::Reserved)),
                    "{}",
                    case["id"]
                );
            }
            assert!(
                check_branch_reservations(
                    repository_root,
                    branch,
                    same_claim.operation_id,
                    same_claim.submission_key,
                    &reservations,
                )
                .is_ok()
            );
            checked += 1;
        }
        assert_eq!(checked, 3);
    }

    #[test]
    fn branch_summary_order_defaults_and_paths_match_reference() {
        let mut checked = 0;
        for case in cases("list-branches") {
            let input = &case["input"];
            let mut output = Vec::new();
            for worktree in input["worktrees"].as_array().unwrap() {
                let path = worktree["path"].as_str().unwrap();
                let branch = worktree["branch"].as_str().unwrap();
                output.extend_from_slice(format!("worktree {path}\0HEAD hash\0").as_bytes());
                if branch == "(detached)" {
                    output.extend_from_slice(b"detached\0");
                } else {
                    output.extend_from_slice(format!("branch refs/heads/{branch}\0").as_bytes());
                }
                output.push(0);
            }
            let worktrees = parse_worktrees(&output).unwrap();
            let origin_head = input["originHead"]
                .as_str()
                .map(|reference| format!("refs/remotes/{}", reference.trim_end()));
            let actual = branch_summaries(
                input["refOutput"].as_str().unwrap().as_bytes(),
                input["currentBranch"].as_str(),
                origin_head.as_deref(),
                &worktrees,
            )
            .unwrap();
            let expected = &case["expected"];
            assert_eq!(
                actual.default.as_deref(),
                expected["defaultBranch"].as_str()
            );
            let expected_branches = expected["branches"].as_array().unwrap();
            assert_eq!(actual.branches.len(), expected_branches.len());
            for (branch, expected) in actual.branches.iter().zip(expected_branches) {
                assert_eq!(Some(branch.name.as_str()), expected["name"].as_str());
                assert_eq!(Some(branch.current), expected["current"].as_bool());
                assert_eq!(
                    Some(branch.commit_timestamp),
                    expected["commitTimestamp"].as_i64()
                );
                assert_eq!(
                    branch.checked_out_path.as_deref(),
                    expected["checkedOutPath"].as_str().map(Path::new)
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 2);
    }
}
