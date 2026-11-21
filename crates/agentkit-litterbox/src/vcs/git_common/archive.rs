//! Materialising a Git tree as a tar archive, and unpacking one.

use std::path::{Path, PathBuf};

use git2::{ObjectType, Repository};

use super::GitError;
use crate::domain::SandboxError;

/// The tree a reference points at.
pub fn tree_from_reference<'a>(
    repo: &'a Repository,
    reference: &str,
) -> Result<git2::Tree<'a>, SandboxError> {
    let obj = repo
        .revparse_single(reference)
        .map_err(|source| SandboxError::from(GitError::Archive { source }))?;
    obj.peel_to_tree()
        .map_err(|source| SandboxError::from(GitError::Archive { source }))
}

/// Append every entry of `tree` to `builder`, under `base`.
pub fn append_tree(
    repo: &Repository,
    builder: &mut tar::Builder<Vec<u8>>,
    tree: &git2::Tree<'_>,
    base: &Path,
) -> Result<(), SandboxError> {
    for entry in tree.iter() {
        append_entry(repo, builder, base, &entry)?;
    }

    Ok(())
}

fn append_entry(
    repo: &Repository,
    builder: &mut tar::Builder<Vec<u8>>,
    base: &Path,
    entry: &git2::TreeEntry<'_>,
) -> Result<(), SandboxError> {
    let name = entry
        .name()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid path"))?;
    let path = base.join(PathBuf::from(name));

    match entry.kind() {
        Some(ObjectType::Tree) => {
            let subtree = entry
                .to_object(repo)
                .map_err(|source| SandboxError::from(GitError::Archive { source }))?
                .peel_to_tree()
                .map_err(|source| SandboxError::from(GitError::Archive { source }))?;
            append_tree(repo, builder, &subtree, &path)
        }
        Some(ObjectType::Blob) => append_blob(repo, builder, &path, entry),
        _ => Ok(()),
    }
}

fn append_blob(
    repo: &Repository,
    builder: &mut tar::Builder<Vec<u8>>,
    path: &Path,
    entry: &git2::TreeEntry<'_>,
) -> Result<(), SandboxError> {
    let blob = entry
        .to_object(repo)
        .map_err(|source| SandboxError::from(GitError::Archive { source }))?
        .peel_to_blob()
        .map_err(|source| SandboxError::from(GitError::Archive { source }))?;

    let mut header = tar::Header::new_gnu();
    let mode = match entry.filemode() {
        0 => 0o644,
        value => value as u32,
    };
    let size = u64::try_from(blob.size())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "blob too large"))?;
    header.set_size(size);
    header.set_mode(mode);
    header.set_cksum();

    builder.append_data(&mut header, path, blob.content())?;
    Ok(())
}

/// The archive of the tree `reference` points at.
pub fn archive(repo: &Repository, reference: &str) -> Result<Vec<u8>, SandboxError> {
    let tree = tree_from_reference(repo, reference)?;
    let mut builder = tar::Builder::new(Vec::new());

    append_tree(repo, &mut builder, &tree, Path::new(""))?;

    builder.into_inner().map_err(SandboxError::Io)
}

/// Unpack an archive into `into`, refusing paths that escape it.
pub fn unpack(bytes: &[u8], into: &Path) -> Result<(), SandboxError> {
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    archive.unpack(into).map_err(SandboxError::Io)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git2::{IndexAddOption, Repository, Signature};
    use tempfile::TempDir;

    use super::*;

    fn committed_repo() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        let repo = Repository::init(dir.path()).expect("init");
        fs::write(dir.path().join("greeting.txt"), "hello\n").expect("write");
        fs::create_dir(dir.path().join("nested")).expect("mkdir");
        fs::write(dir.path().join("nested").join("note.txt"), "note\n").expect("write");

        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_oid).expect("find tree");
        let signature = Signature::now("test", "test@example.com").expect("signature");
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "commit",
            &tree,
            &parents,
        )
        .expect("commit");

        dir
    }

    #[test]
    fn an_archive_round_trips_through_unpack() {
        let source = committed_repo();
        let repo = Repository::open(source.path()).expect("open");
        let bytes = archive(&repo, "HEAD").expect("archive");
        let output = TempDir::new().expect("output");

        unpack(&bytes, output.path()).expect("unpack");

        assert_eq!(
            fs::read_to_string(output.path().join("greeting.txt")).expect("read"),
            "hello\n"
        );
        assert_eq!(
            fs::read_to_string(output.path().join("nested").join("note.txt")).expect("read"),
            "note\n"
        );
    }

    #[test]
    fn an_unknown_reference_is_an_archive_error() {
        let source = committed_repo();
        let repo = Repository::open(source.path()).expect("open");

        let error = archive(&repo, "no-such-reference").expect_err("missing reference");

        assert!(error.to_string().contains("Git archive failed"));
    }
}
