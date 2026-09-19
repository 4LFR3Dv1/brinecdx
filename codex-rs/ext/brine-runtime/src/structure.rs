use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use codex_brine_runtime::StructuralSymbol;
use codex_brine_runtime::UnresolvedStructuralRelation;
use codex_brine_runtime::WorkspaceStructureObservation;
use sha2::Digest;
use sha2::Sha256;

const SOURCE_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "js", "jsx"];

pub(crate) fn observe_structure(root: &Path) -> Result<WorkspaceStructureObservation, String> {
    let paths = source_paths(root)?;
    let known_paths: BTreeSet<String> = paths.iter().cloned().collect();
    let mut symbols = Vec::new();
    let mut unresolved_relations = Vec::new();

    for path in paths {
        let absolute = root.join(&path);
        if !absolute.is_file() {
            continue;
        }
        let bytes = fs::read(&absolute)
            .map_err(|error| format!("failed reading structural path {}: {error}", absolute.display()))?;
        let source = String::from_utf8_lossy(&bytes);
        let extension = absolute
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        match extension.as_str() {
            "rs" => index_rust(
                &path,
                &source,
                &known_paths,
                &mut symbols,
                &mut unresolved_relations,
            ),
            "ts" | "tsx" | "js" | "jsx" => index_typescript_like(
                root,
                &path,
                &source,
                &known_paths,
                &mut symbols,
                &mut unresolved_relations,
            ),
            _ => {}
        }
    }

    symbols.sort_by(|left, right| {
        (&left.path, left.line, &left.kind, &left.name)
            .cmp(&(&right.path, right.line, &right.kind, &right.name))
    });
    unresolved_relations.sort_by(|left, right| {
        (&left.path, left.line, &left.kind, &left.target)
            .cmp(&(&right.path, right.line, &right.kind, &right.target))
    });

    let mut basis = Vec::new();
    for symbol in &symbols {
        push_field(&mut basis, &symbol.path);
        push_field(&mut basis, &symbol.line.to_string());
        push_field(&mut basis, &symbol.kind);
        push_field(&mut basis, &symbol.name);
    }
    basis.push(0xff);
    for relation in &unresolved_relations {
        push_field(&mut basis, &relation.path);
        push_field(&mut basis, &relation.line.to_string());
        push_field(&mut basis, &relation.kind);
        push_field(&mut basis, &relation.target);
    }

    Ok(WorkspaceStructureObservation {
        digest: sha256_hex(&basis),
        symbols,
        unresolved_relations,
    })
}

pub(crate) fn is_source_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            SOURCE_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn source_paths(root: &Path) -> Result<Vec<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .map_err(|error| format!("failed to launch git ls-files: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut paths: Vec<String> = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| String::from_utf8_lossy(value).replace('\\', "/"))
        .filter(|path| is_source_path(path))
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn index_rust(
    path: &str,
    source: &str,
    known_paths: &BTreeSet<String>,
    symbols: &mut Vec<StructuralSymbol>,
    unresolved: &mut Vec<UnresolvedStructuralRelation>,
) {
    for (index, line) in source.lines().enumerate() {
        let line_number = (index + 1) as u32;
        let normalized = strip_rust_prefixes(line.trim_start());
        for (keyword, kind) in [
            ("fn ", "function"),
            ("struct ", "struct"),
            ("enum ", "enum"),
            ("trait ", "trait"),
            ("type ", "type"),
            ("const ", "const"),
            ("static ", "static"),
            ("mod ", "module"),
        ] {
            if let Some(rest) = normalized.strip_prefix(keyword)
                && let Some(name) = identifier(rest)
            {
                symbols.push(StructuralSymbol {
                    path: path.to_owned(),
                    name: name.to_owned(),
                    kind: kind.to_owned(),
                    line: line_number,
                });
                if keyword == "mod " && normalized.trim_end().ends_with(';') {
                    let candidates = rust_module_candidates(path, name);
                    if !candidates.iter().any(|candidate| known_paths.contains(candidate)) {
                        unresolved.push(UnresolvedStructuralRelation {
                            path: path.to_owned(),
                            kind: "module".to_owned(),
                            target: name.to_owned(),
                            line: line_number,
                        });
                    }
                }
                break;
            }
        }
    }
}

fn index_typescript_like(
    root: &Path,
    path: &str,
    source: &str,
    known_paths: &BTreeSet<String>,
    symbols: &mut Vec<StructuralSymbol>,
    unresolved: &mut Vec<UnresolvedStructuralRelation>,
) {
    for (index, line) in source.lines().enumerate() {
        let line_number = (index + 1) as u32;
        let normalized = strip_ts_prefixes(line.trim_start());

        for (keyword, kind) in [
            ("function ", "function"),
            ("class ", "class"),
            ("interface ", "interface"),
            ("type ", "type"),
            ("enum ", "enum"),
            ("const ", "const"),
            ("let ", "variable"),
            ("var ", "variable"),
        ] {
            if let Some(rest) = normalized.strip_prefix(keyword)
                && let Some(name) = identifier(rest)
            {
                symbols.push(StructuralSymbol {
                    path: path.to_owned(),
                    name: name.to_owned(),
                    kind: kind.to_owned(),
                    line: line_number,
                });
                break;
            }
        }

        if let Some(target) = import_target(normalized)
            && !import_resolves(root, path, target, known_paths)
        {
            unresolved.push(UnresolvedStructuralRelation {
                path: path.to_owned(),
                kind: if target.starts_with('.') {
                    "import".to_owned()
                } else {
                    "external_import".to_owned()
                },
                target: target.to_owned(),
                line: line_number,
            });
        }
    }
}

fn strip_rust_prefixes(mut line: &str) -> &str {
    loop {
        let before = line;
        line = line.trim_start();
        if let Some(rest) = line.strip_prefix("pub ") {
            line = rest;
        } else if line.starts_with("pub(") {
            if let Some(end) = line.find(')') {
                line = &line[end + 1..];
            }
        } else if let Some(rest) = line.strip_prefix("async ") {
            line = rest;
        } else if let Some(rest) = line.strip_prefix("unsafe ") {
            line = rest;
        } else {
            break;
        }
        if line == before {
            break;
        }
    }
    line.trim_start()
}

fn strip_ts_prefixes(mut line: &str) -> &str {
    loop {
        let before = line;
        for prefix in ["export ", "default ", "declare ", "async ", "abstract "] {
            if let Some(rest) = line.strip_prefix(prefix) {
                line = rest.trim_start();
                break;
            }
        }
        if line == before {
            break;
        }
    }
    line
}

fn identifier(value: &str) -> Option<&str> {
    let end = value
        .char_indices()
        .take_while(|(_, character)| character.is_ascii_alphanumeric() || *character == '_' || *character == '$')
        .last()
        .map(|(index, character)| index + character.len_utf8())?;
    let candidate = &value[..end];
    (!candidate.is_empty()).then_some(candidate)
}

fn rust_module_candidates(path: &str, name: &str) -> Vec<String> {
    let source = Path::new(path);
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let file_name = source.file_name().and_then(|value| value.to_str()).unwrap_or_default();
    let module_root = if matches!(file_name, "lib.rs" | "main.rs" | "mod.rs") {
        parent.to_path_buf()
    } else {
        let stem = source.file_stem().and_then(|value| value.to_str()).unwrap_or_default();
        parent.join(stem)
    };
    vec![
        slash_path(module_root.join(format!("{name}.rs"))),
        slash_path(module_root.join(name).join("mod.rs")),
    ]
}

fn import_target(line: &str) -> Option<&str> {
    if let Some(index) = line.find(" from ") {
        return quoted_value(&line[index + 6..]);
    }
    if let Some(rest) = line.strip_prefix("import ") {
        return quoted_value(rest);
    }
    if let Some(index) = line.find("require(") {
        return quoted_value(&line[index + 8..]);
    }
    None
}

fn quoted_value(value: &str) -> Option<&str> {
    let value = value.trim_start();
    let quote = value.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &value[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

fn import_resolves(
    root: &Path,
    source_path: &str,
    target: &str,
    known_paths: &BTreeSet<String>,
) -> bool {
    if !target.starts_with('.') {
        return false;
    }
    let source_parent = Path::new(source_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let base = normalize_relative(root, source_parent.join(target));
    let candidates = if Path::new(target).extension().is_some() {
        vec![base]
    } else {
        let mut values = Vec::new();
        for extension in ["ts", "tsx", "js", "jsx"] {
            values.push(format!("{base}.{extension}"));
            values.push(format!("{base}/index.{extension}"));
        }
        values
    };
    candidates.iter().any(|candidate| known_paths.contains(candidate))
}

fn normalize_relative(root: &Path, path: PathBuf) -> String {
    let absolute = root.join(path);
    let normalized = absolute
        .components()
        .fold(PathBuf::new(), |mut output, component| {
            use std::path::Component;
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    output.pop();
                }
                other => output.push(other.as_os_str()),
            }
            output
        });
    normalized
        .strip_prefix(root)
        .map(slash_path)
        .unwrap_or_else(|_| slash_path(normalized))
}

fn slash_path(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().replace('\\', "/")
}

fn push_field(buffer: &mut Vec<u8>, value: &str) {
    buffer.extend_from_slice(value.as_bytes());
    buffer.push(0);
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::index_rust;
    use super::index_typescript_like;

    #[test]
    fn rust_index_records_symbols_and_unresolved_modules() {
        let mut symbols = Vec::new();
        let mut unresolved = Vec::new();
        let known = BTreeSet::new();
        index_rust(
            "src/lib.rs",
            "pub struct Engine {}\npub async fn run() {}\nmod missing;\n",
            &known,
            &mut symbols,
            &mut unresolved,
        );
        assert_eq!(
            symbols.iter().map(|symbol| symbol.name.as_str()).collect::<Vec<_>>(),
            vec!["Engine", "run", "missing"]
        );
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].target, "missing");
    }

    #[test]
    fn typescript_index_records_symbols_and_unresolved_imports() {
        let mut symbols = Vec::new();
        let mut unresolved = Vec::new();
        let known = BTreeSet::new();
        index_typescript_like(
            std::path::Path::new("."),
            "src/index.ts",
            "import { x } from './missing';\nexport async function run() {}\ninterface State {}\n",
            &known,
            &mut symbols,
            &mut unresolved,
        );
        assert_eq!(
            symbols.iter().map(|symbol| symbol.name.as_str()).collect::<Vec<_>>(),
            vec!["run", "State"]
        );
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].target, "./missing");
    }
}
