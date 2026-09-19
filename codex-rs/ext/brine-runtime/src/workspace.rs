use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use codex_brine_runtime::WorkspaceMaterialObservation;

use crate::LocalWorkspace;

/// Observe the current local Git material without sending filesystem handles to the runtime.
///
/// Clean tracked files are represented by HEAD. Only changed/untracked paths are content-digested,
/// so an unchanged workspace does not require a full repository rehash.
pub fn observe_local_workspace(
    workspace: &LocalWorkspace,
) -> Result<Option<WorkspaceMaterialObservation>, String> {
    let root = workspace.root.as_path();
    let inside = git_output(root, &["rev-parse", "--is-inside-work-tree"])?;
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
            hash_bytes(root, &fs::read(&absolute).map_err(|error| {
                format!("failed reading changed path {}: {error}", absolute.display())
            })?)?
        } else {
            "missing".to_owned()
        };
        file_digests.insert(path.clone(), digest);
    }

    let index_state = hash_bytes(root, &index_raw)?;
    let mut working_basis = working_raw;
    for (path, digest) in &file_digests {
        working_basis.extend_from_slice(path.as_bytes());
        working_basis.push(0);
        working_basis.extend_from_slice(digest.as_bytes());
        working_basis.push(0);
    }
    let working_tree = hash_bytes(root, &working_basis)?;

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
    let material_digest = hash_bytes(root, &material_basis)?;

    Ok(Some(WorkspaceMaterialObservation {
        head,
        index_state,
        working_tree,
        changed_paths,
        file_digests,
        material_digest,
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

fn hash_bytes(root: &Path, bytes: &[u8]) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["hash-object", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to launch git hash-object: {error}"))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "git hash-object stdin unavailable".to_owned())?
        .write_all(bytes)
        .map_err(|error| format!("failed writing git hash-object stdin: {error}"))?;

    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed waiting for git hash-object: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(trim_ascii(&output.stdout)).into_owned())
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

    use super::observe_local_workspace;
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
