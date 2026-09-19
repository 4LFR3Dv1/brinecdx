use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use codex_brine_runtime::WorkspaceMaterialObservation;
use sha2::Digest;
use sha2::Sha256;

use crate::LocalWorkspace;
use crate::structure::is_source_path;
use crate::structure::observe_structure;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceIdentity {
    pub workspace_key: String,
    pub workspace_aliases: Vec<String>,
    pub repository_identity: String,
}

/// Resolve stable local workspace identity without sending host paths or remote URLs to the runtime.
///
/// Worktrees that share one Git common dir resolve to the same workspace key. When an origin exists,
/// repository identity is a digest of that URL so embedded credentials are never persisted in clear.
pub fn identify_local_workspace(root: &Path) -> WorkspaceIdentity {
    let fallback_root = canonical_or_original(root);
    let common_dir = git_output_optional(root, &["rev-parse", "--git-common-dir"])
        .and_then(|bytes| path_from_git_output(root, &bytes))
        .map(|path| canonical_or_original(&path))
        .unwrap_or_else(|| fallback_root.clone());

    let workspace_key = format!(
        "local-git:{}",
        sha256_hex(common_dir.to_string_lossy().as_bytes())
    );

    let legacy_workspace_key = format!(
        "local:{}",
        sha256_hex(root.to_string_lossy().as_bytes())
    );

    let repository_identity = git_output_optional(root, &["config", "--get", "remote.origin.url"])
        .and_then(|bytes| {
            let value = String::from_utf8_lossy(trim_ascii(&bytes)).into_owned();
            (!value.is_empty()).then_some(value)
        })
        .map(|origin| format!("origin-sha256:{}", sha256_hex(origin.as_bytes())))
        .unwrap_or_else(|| workspace_key.clone());

    let workspace_aliases = (legacy_workspace_key != workspace_key)
        .then_some(legacy_workspace_key)
        .into_iter()
        .collect();

    WorkspaceIdentity {
        workspace_key,
        workspace_aliases,
        repository_identity,
    }
}

/// Observe the current local Git material without sending filesystem handles to the runtime.
///
/// Clean tracked files are represented by HEAD. Only changed/untracked paths are content-digested,
/// so an unchanged workspace does not require a full repository rehash.
pub fn observe_local_workspace(
    workspace: &LocalWorkspace,
) -> Result<Option<WorkspaceMaterialObservation>, String> {
    observe_local_workspace_with_previous(workspace, None)
}

pub(crate) fn observe_local_workspace_with_previous(
    workspace: &LocalWorkspace,
    previous: Option<&WorkspaceMaterialObservation>,
) -> Result<Option<WorkspaceMaterialObservation>, String> {
    let root = workspace.root.as_path();
    let Some(inside) = git_output_optional(root, &["rev-parse", "--is-inside-work-tree"]) else {
        return Ok(None);
    };
    if trim_ascii(&inside) != b"true" {
        return Ok(None);
    }

    let head = git_output_optional(root, &["rev-parse", "--verify", "HEAD"])
        .map(|bytes| String::from_utf8_lossy(trim_ascii(&bytes)).into_owned());

    let index_raw = git_output(root, &["diff", "--cached", "--raw", "-z", "--no-abbrev"])?;
    let working_raw = git_output(root, &["diff", "--raw", "-z", "--no-abbrev"])?;

    let mut changed_paths = Vec::new();
    for args in [
        &["diff", "--name-only", "-z"][..],
        &["diff", "--cached", "--name-only", "-z"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        changed_paths.extend(parse_nul_paths(&git_output(root, args)?));
    }
    changed_paths.sort();
    changed_paths.dedup();

    let mut file_digests = BTreeMap::new();
    for path in &changed_paths {
        let absolute = root.join(path);
        let digest = if absolute.is_file() {
            hash_bytes(&fs::read(&absolute).map_err(|error| {
                format!("failed reading changed path {}: {error}", absolute.display())
            })?)?
        } else {
            "missing".to_owned()
        };
        file_digests.insert(path.clone(), digest);
    }

    let index_state = hash_bytes(&index_raw)?;
    let mut working_basis = working_raw;
    for (path, digest) in &file_digests {
        working_basis.extend_from_slice(path.as_bytes());
        working_basis.push(0);
        working_basis.extend_from_slice(digest.as_bytes());
        working_basis.push(0);
    }
    let working_tree = hash_bytes(&working_basis)?;

    let mut material_basis = Vec::new();
    if let Some(head) = &head {
        material_basis.extend_from_slice(head.as_bytes());
    }
    material_basis.push(0);
    material_basis.extend_from_slice(index_state.as_bytes());
    material_basis.push(0);
    material_basis.extend_from_slice(working_tree.as_bytes());
    material_basis.push(0);
    for path in &changed_paths {
        material_basis.extend_from_slice(path.as_bytes());
        material_basis.push(0);
        if let Some(digest) = file_digests.get(path) {
            material_basis.extend_from_slice(digest.as_bytes());
        }
        material_basis.push(0);
    }
    let material_digest = hash_bytes(&material_basis)?;
    let structure = previous
        .filter(|previous| {
            previous.material_digest == material_digest
                || (previous.head == head && !changed_paths.iter().any(|path| is_source_path(path)))
        })
        .map(|previous| previous.structure.clone())
        .map(Ok)
        .unwrap_or_else(|| observe_structure(root))?;

    Ok(Some(WorkspaceMaterialObservation {
        head,
        index_state,
        working_tree,
        changed_paths,
        file_digests,
        material_digest,
        structure,
    }))
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to launch git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_output_optional(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn hash_bytes(bytes: &[u8]) -> Result<String, String> {
    Ok(sha256_hex(bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_from_git_output(root: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let raw = trim_ascii(bytes);
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8_lossy(raw).into_owned());
    Some(if path.is_absolute() { path } else { root.join(path) })
}

fn parse_nul_paths(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use super::identify_local_workspace;
    use super::observe_local_workspace;
    use super::observe_local_workspace_with_previous;
    use crate::LocalWorkspace;

    #[test]
    fn observation_is_stable_until_material_changes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "brine-workspace-observer-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp repository");
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "brine@example.invalid"]);
        run_git(&root, &["config", "user.name", "Brine Test"]);

        let tracked = root.join("tracked.txt");
        fs::write(&tracked, "one\n").expect("write tracked file");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);

        let workspace = LocalWorkspace {
            root: root.clone(),
            repository_identity: "test-repository".to_owned(),
        };
        let first = observe_local_workspace(&workspace)
            .expect("observe first snapshot")
            .expect("git workspace");
        let same = observe_local_workspace(&workspace)
            .expect("observe unchanged snapshot")
            .expect("git workspace");
        assert_eq!(same.material_digest, first.material_digest);
        assert_eq!(same.changed_paths, first.changed_paths);

        fs::write(&tracked, "two\n").expect("change tracked file");
        let changed = observe_local_workspace(&workspace)
            .expect("observe changed snapshot")
            .expect("git workspace");
        assert_ne!(changed.material_digest, first.material_digest);
        assert_eq!(changed.changed_paths, vec!["tracked.txt".to_owned()]);
        assert!(changed.file_digests.contains_key("tracked.txt"));

        fs::remove_dir_all(root).expect("remove temp repository");
    }



    #[test]
    fn structural_index_is_reused_until_code_material_changes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "brine-workspace-structure-cache-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create source directory");
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "brine@example.invalid"]);
        run_git(&root, &["config", "user.name", "Brine Test"]);
        fs::write(root.join("src/lib.rs"), "pub fn run() {}\n").expect("write source");
        fs::write(root.join("README.md"), "one\n").expect("write readme");
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-m", "initial"]);

        let workspace = LocalWorkspace {
            root: root.clone(),
            repository_identity: "test-repository".to_owned(),
        };
        let mut first = observe_local_workspace(&workspace)
            .expect("observe initial workspace")
            .expect("git workspace");
        assert!(first.structure.symbols.iter().any(|symbol| symbol.name == "run"));

        first.structure.digest = "cached-structure".to_owned();
        fs::write(root.join("README.md"), "two\n").expect("change non-source file");
        let docs_changed = observe_local_workspace_with_previous(&workspace, Some(&first))
            .expect("observe docs change")
            .expect("git workspace");
        assert_ne!(docs_changed.material_digest, first.material_digest);
        assert_eq!(docs_changed.structure.digest, "cached-structure");

        fs::write(
            root.join("src/lib.rs"),
            "pub fn run() {}\npub struct State;\n",
        )
        .expect("change source file");
        let code_changed = observe_local_workspace_with_previous(&workspace, Some(&docs_changed))
            .expect("observe source change")
            .expect("git workspace");
        assert_ne!(code_changed.structure.digest, "cached-structure");
        assert!(
            code_changed
                .structure
                .symbols
                .iter()
                .any(|symbol| symbol.name == "State")
        );

        fs::remove_dir_all(root).expect("remove temp repository");
    }

    #[test]
    fn linked_worktree_shares_workspace_identity_without_leaking_origin() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "brine-workspace-identity-{}-{nonce}",
            std::process::id()
        ));
        let root = base.join("main");
        let linked = base.join("linked");
        fs::create_dir_all(&root).expect("create temp repository");
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "brine@example.invalid"]);
        run_git(&root, &["config", "user.name", "Brine Test"]);
        fs::write(root.join("tracked.txt"), "one\n").expect("write tracked file");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);
        run_git(
            &root,
            &[
                "remote",
                "add",
                "origin",
                "https://user:supersecret@example.invalid/org/repo.git",
            ],
        );

        let linked_arg = linked.to_string_lossy().into_owned();
        run_git(&root, &["worktree", "add", "-b", "linked-test", &linked_arg]);

        let main_identity = identify_local_workspace(&root);
        let linked_identity = identify_local_workspace(&linked);

        assert_eq!(main_identity.workspace_key, linked_identity.workspace_key);
        assert_eq!(
            main_identity.repository_identity,
            linked_identity.repository_identity
        );
        assert!(main_identity.repository_identity.starts_with("origin-sha256:"));
        assert!(!main_identity.repository_identity.contains("supersecret"));
        assert!(!main_identity.repository_identity.contains("example.invalid"));

        run_git(&root, &["worktree", "remove", "--force", &linked_arg]);
        fs::remove_dir_all(base).expect("remove temp repository");
    }

    fn run_git(root: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("launch git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
