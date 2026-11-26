//! Resolve the project behind a card without merging independent Git checkouts.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use matou_native_core::git_repo::Repository;

pub(crate) struct ProjectScope {
    project_root: PathBuf,
    roots: Vec<PathBuf>,
}

impl ProjectScope {
    /// Run on the catalog worker: repository discovery invokes the existing scoped Git helpers.
    pub(crate) fn discover(cwd: &Path, workspace_root: Option<&Path>) -> io::Result<Self> {
        let cwd = resolve(cwd)?;
        if !cwd.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Card directory is unavailable",
            ));
        }
        if let Some(repository) = Repository::discover(&cwd).ok().flatten() {
            let mut roots = vec![repository.root().to_owned()];
            let mut project_root = repository.root().to_owned();
            let expected_common = common_directory(repository.root());
            if let Ok(worktrees) = repository.worktrees() {
                for worktree in worktrees.into_iter().filter(|entry| !entry.bare) {
                    let Ok(root) = resolve(&worktree.path) else {
                        continue;
                    };
                    // A stale registration can point at a directory now owned by another repository.
                    if root.exists()
                        && (expected_common.is_none() || common_directory(&root) != expected_common)
                    {
                        continue;
                    }
                    if worktree.main {
                        project_root = root.clone();
                    }
                    roots.push(root);
                }
            }
            roots.sort();
            roots.dedup();
            return Ok(Self {
                project_root,
                roots,
            });
        }

        let project_root = workspace_root
            .and_then(|root| resolve(root).ok())
            .filter(|root| root.is_dir() && cwd.starts_with(root))
            .filter(|root| matches!(Repository::discover(root), Ok(None)))
            .unwrap_or(cwd);
        Ok(Self {
            roots: vec![project_root.clone()],
            project_root,
        })
    }

    pub(crate) fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Canonical roots, including registered worktrees whose directory has since been removed.
    pub(crate) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub(crate) fn contains(&self, origin: &Path) -> bool {
        let Ok(origin) = resolve(origin) else {
            return false;
        };
        // Prefer a nested registered worktree over the main checkout containing it.
        let Some(root) = self
            .roots
            .iter()
            .filter(|root| origin.starts_with(root))
            .max_by_key(|root| root.components().count())
        else {
            return false;
        };

        for ancestor in origin.ancestors().take_while(|ancestor| *ancestor != root) {
            match fs::symlink_metadata(ancestor.join(".git")) {
                Ok(_) => return false,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return false,
            }
        }
        true
    }
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    // Normalize components first so a missing directory followed by `..` cannot defeat containment.
    let path = super::absolute(path)?;
    Repository::registered_worktree_path(&path).map_err(io::Error::other)
}

/// Verify registered worktrees with their on-disk Git pointers, without a process per worktree.
fn common_directory(checkout: &Path) -> Option<PathBuf> {
    let marker = checkout.join(".git");
    let git_directory = if marker.is_dir() {
        fs::canonicalize(marker).ok()?
    } else {
        let pointer = small_file(&marker)?;
        let target = pointer.trim().strip_prefix("gitdir: ")?;
        fs::canonicalize(checkout.join(target)).ok()?
    };
    let common_pointer = git_directory.join("commondir");
    match fs::metadata(&common_pointer) {
        Ok(_) => fs::canonicalize(git_directory.join(small_file(&common_pointer)?.trim())).ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(git_directory),
        Err(_) => None,
    }
}

fn small_file(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8192 {
        return None;
    }
    fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use matou_native_core::git_repo::git_command;

    fn git(cwd: &Path, args: &[&str]) {
        let output = git_command(cwd).unwrap().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository(path: &Path) {
        fs::create_dir_all(path).unwrap();
        git(path, &["init", "--quiet"]);
        git(
            path,
            &[
                "-c",
                "user.name=Scope Test",
                "-c",
                "user.email=scope@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
    }

    #[test]
    fn linked_worktrees_share_project_but_nested_repositories_and_siblings_do_not() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let linked = temp.path().join("linked");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                linked.to_str().unwrap(),
            ],
        );
        let nested_worktree = main.join(".claude/worktrees/feature");
        fs::create_dir_all(nested_worktree.parent().unwrap()).unwrap();
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                nested_worktree.to_str().unwrap(),
            ],
        );
        let nested_repository = main.join("vendor/independent");
        repository(&nested_repository);
        let sibling = temp.path().join("project-other");
        repository(&sibling);
        let child = linked.join("src");
        fs::create_dir(&child).unwrap();

        let scope = ProjectScope::discover(&child, Some(&main)).unwrap();
        assert_eq!(scope.project_root(), fs::canonicalize(&main).unwrap());
        assert_eq!(scope.roots().len(), 3);
        assert!(scope.contains(&main));
        assert!(scope.contains(&main.join("removed/subdirectory")));
        assert!(scope.contains(&nested_worktree.join("src")));
        assert!(scope.contains(&linked));
        assert!(!scope.contains(&nested_repository));
        assert!(!scope.contains(&nested_repository.join("src")));
        assert!(!scope.contains(&sibling));
        assert!(!scope.contains(&main.join("removed/../../project-other")));
    }

    #[test]
    fn active_nested_repository_keeps_its_own_project() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let nested = main.join("nested");
        repository(&nested);
        let scope = ProjectScope::discover(&nested, Some(&main)).unwrap();
        assert_eq!(scope.project_root(), fs::canonicalize(&nested).unwrap());
        assert!(!scope.contains(&main));
    }

    #[test]
    fn stale_registration_cannot_admit_a_replacement_repository() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let replaced = temp.path().join("replaced");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                replaced.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&replaced).unwrap();
        repository(&replaced);
        let retired = temp.path().join("retired");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                retired.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&retired).unwrap();

        let scope = ProjectScope::discover(&main, None).unwrap();
        assert!(!scope.contains(&replaced));
        assert!(scope.contains(&retired.join("src")));
        assert_eq!(scope.roots().len(), 2);
    }

    #[test]
    fn plain_workspace_includes_subdirectories_without_crossing_git_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let child = workspace.join("src");
        fs::create_dir_all(&child).unwrap();
        let nested = workspace.join("nested");
        repository(&nested);
        let scope = ProjectScope::discover(&child, Some(&workspace)).unwrap();
        assert_eq!(scope.project_root(), fs::canonicalize(&workspace).unwrap());
        assert!(scope.contains(&workspace.join("removed")));
        assert!(!scope.contains(&nested));
        assert!(!scope.contains(&temp.path().join("workspace-other")));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_resolve_to_the_same_project() {
        let temp = tempfile::tempdir().unwrap();
        let main = temp.path().join("project");
        repository(&main);
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&main, &alias).unwrap();
        let scope = ProjectScope::discover(&alias, None).unwrap();
        assert!(scope.contains(&alias.join("removed")));
        assert_eq!(scope.project_root(), fs::canonicalize(&main).unwrap());
    }
}
