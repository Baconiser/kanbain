//! Reading repositories for the code index (E16, E17, D45): the files at a
//! revision with their blob SHAs, their contents in one `git cat-file`
//! batch, and the files a ticket branch changed.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoFile {
    pub path: String,
    pub blob_sha: String,
    pub size: usize,
}

/// `git` in `repo`, without a console window. KanbAIn is a GUI app on
/// Windows, where every console child pops up a terminal otherwise, and
/// indexing runs git on every start and every ticket change.
fn git_command(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = git_command(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The revision to index: the repo's default target branch when it
/// exists, else `HEAD`.
pub fn main_revision(repo: &Path, default_branch: Option<&str>) -> Result<String, String> {
    let candidates = default_branch.into_iter().chain(["main", "master", "HEAD"]);
    for candidate in candidates {
        if let Ok(sha) = git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("{candidate}^{{commit}}"),
            ],
        ) {
            return Ok(sha.trim().to_string());
        }
    }
    Err(format!("{} has no commits", repo.display()))
}

/// Regular files at `rev` (`git ls-tree -r -l`).
pub fn list_files(repo: &Path, rev: &str) -> Result<Vec<RepoFile>, String> {
    let out = git(repo, &["ls-tree", "-r", "-l", "-z", rev])?;
    Ok(out
        .split('\0')
        .filter_map(|entry| {
            // <mode> SP <type> SP <sha> SP+ <size> TAB <path>
            let (meta, path) = entry.split_once('\t')?;
            let mut parts = meta.split_whitespace();
            let _mode = parts.next()?;
            let kind = parts.next()?;
            let sha = parts.next()?;
            let size = parts.next()?.parse().ok()?;
            (kind == "blob").then(|| RepoFile {
                path: path.to_string(),
                blob_sha: sha.to_string(),
                size,
            })
        })
        .collect())
}

/// Blob contents by SHA, from one `git cat-file --batch`.
pub fn read_blobs(repo: &Path, shas: &[String]) -> Result<HashMap<String, Vec<u8>>, String> {
    if shas.is_empty() {
        return Ok(HashMap::new());
    }
    let mut child = git_command(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("git cat-file: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let input: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
    let mut blobs = HashMap::new();
    for _ in shas {
        let mut header = String::new();
        if reader.read_line(&mut header).map_err(|e| e.to_string())? == 0 {
            break;
        }
        // <sha> <type> <size>, or <sha> missing
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() != 3 {
            continue;
        }
        let size: usize = parts[2].parse().map_err(|_| "bad cat-file header")?;
        let mut body = vec![0; size + 1];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        body.truncate(size);
        blobs.insert(parts[0].to_string(), body);
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(blobs)
}

/// Files that `head` changed since it left `base` (`base...head`), with
/// their blob at `head`; deleted files have no blob.
pub fn changed_files(
    repo: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<(String, Option<RepoFile>)>, String> {
    let out = git(
        repo,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &format!("{base}...{head}"),
        ],
    )?;
    let paths: Vec<&str> = out.split('\0').filter(|p| !p.is_empty()).collect();
    if paths.is_empty() {
        return Ok(vec![]);
    }
    let at_head: HashMap<String, RepoFile> = list_files(repo, head)?
        .into_iter()
        .map(|f| (f.path.clone(), f))
        .collect();
    Ok(paths
        .into_iter()
        .map(|p| (p.to_string(), at_head.get(p).cloned()))
        .collect())
}

/// Whether a branch exists in the repo.
pub fn has_branch(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// UTF-8 text without NUL bytes, or `None` for binaries.
pub fn as_text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn run(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "Dev")
            .env("GIT_AUTHOR_EMAIL", "d@e.x")
            .env("GIT_COMMITTER_NAME", "Dev")
            .env("GIT_COMMITTER_EMAIL", "d@e.x")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }

    #[test]
    fn files_blobs_and_branch_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        run(repo, &["init", "-q", "-b", "main"]);
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/a.rs"), "fn a() {}\n").unwrap();
        fs::write(repo.join("b.txt"), "b\n").unwrap();
        run(repo, &["add", "."]);
        run(repo, &["commit", "-qm", "init"]);
        run(repo, &["checkout", "-qb", "kanbain/T-1-x"]);
        fs::write(repo.join("src/a.rs"), "fn a() { changed() }\n").unwrap();
        fs::remove_file(repo.join("b.txt")).unwrap();
        run(repo, &["add", "-A"]);
        run(repo, &["commit", "-qm", "T-1: change"]);
        run(repo, &["checkout", "-q", "main"]);

        let rev = main_revision(repo, Some("nope")).unwrap();
        let files = list_files(repo, &rev).unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["b.txt", "src/a.rs"]
        );
        let blobs = read_blobs(
            repo,
            &files.iter().map(|f| f.blob_sha.clone()).collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(blobs[&files[1].blob_sha], b"fn a() {}\n");

        assert!(has_branch(repo, "kanbain/T-1-x"));
        let changed = changed_files(repo, &rev, "kanbain/T-1-x").unwrap();
        assert_eq!(changed.len(), 2);
        let a = changed.iter().find(|(p, _)| p == "src/a.rs").unwrap();
        assert_ne!(a.1.as_ref().unwrap().blob_sha, files[1].blob_sha);
        assert!(changed.iter().any(|(p, f)| p == "b.txt" && f.is_none()));
        assert_eq!(as_text(vec![0, 1]), None);
    }
}
