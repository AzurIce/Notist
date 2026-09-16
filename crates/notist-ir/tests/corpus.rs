//! 语料库对拍：`corpus/` 下每个源文件、每个包、每棵依赖树各有一份 golden。
//!
//! 语料只放输入，期望输出放在本 crate 的 `tests/golden/`，按语料相对路径镜像。
//! 这样语料可以被直接当 vault 打开，不会混进生成物。
//!
//! 用 `UPDATE_GOLDEN=1 cargo test -p notist-ir` 重写 golden。

use std::fs;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// 语料里的一个路径，对应 golden 树里的同名 `.dump`。
fn golden_for(path: &Path) -> PathBuf {
    let relative = path
        .strip_prefix(corpus_root())
        .unwrap_or_else(|_| panic!("{} is outside the corpus", path.display()));
    golden_root().join(relative).with_extension("dump")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDEN").is_some()
}

/// 递归收集源文件；`api.not` 是声明模块，内容对拍跳过它。
fn collect_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("readable directory") {
        let path = entry.expect("readable entry").path();
        if path.is_dir() {
            collect_sources(&path, sources);
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !matches!(extension, "not" | "notc") {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some("api.not") {
            continue;
        }
        sources.push(path);
    }
}

#[test]
fn sources_match_golden() {
    let mut sources = Vec::new();
    collect_sources(&corpus_root().join("syntax"), &mut sources);
    collect_sources(&corpus_root().join("vault"), &mut sources);
    sources.sort();
    assert!(!sources.is_empty(), "no corpus sources found");

    for source_path in sources {
        let source = fs::read_to_string(&source_path).expect("readable source");
        let is_code = source_path.extension().and_then(|value| value.to_str()) == Some("notc");
        let result = if is_code {
            notist_ir::compile_code(&source)
        } else {
            notist_ir::compile(&source)
        };
        compare(&golden_for(&source_path), &notist_ir::dump::dump(&result));
    }
}

#[test]
fn packages_match_golden() {
    let root = corpus_root().join("packages");
    let mut directories: Vec<PathBuf> = fs::read_dir(&root)
        .expect("packages directory")
        .filter_map(|entry| {
            let path = entry.expect("readable entry").path();
            path.is_dir().then_some(path)
        })
        .collect();
    directories.sort();
    assert!(!directories.is_empty(), "no package corpus found");

    for directory in directories {
        let report = notist_ir::package::load(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        compare(&golden_for(&directory), &notist_ir::dump::dump_package(&report));
    }
}

#[test]
fn workspaces_match_golden() {
    let root = corpus_root();
    let mut roots: Vec<PathBuf> = fs::read_dir(&root)
        .expect("corpus directory")
        .filter_map(|entry| {
            let path = entry.expect("readable entry").path();
            (path.is_dir() && path.join("Notist.toml").is_file()).then_some(path)
        })
        .collect();
    roots.sort();
    assert!(!roots.is_empty(), "no workspace corpus found");

    for directory in roots {
        let graph = notist_ir::package::load_graph(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        compare(&golden_for(&directory), &notist_ir::dump::dump_graph(&graph));
    }
}

fn compare(golden_path: &Path, actual: &str) {
    if updating() {
        if let Some(parent) = golden_path.parent() {
            fs::create_dir_all(parent).expect("writable golden directory");
        }
        fs::write(golden_path, actual).expect("writable golden");
        return;
    }
    let expected = fs::read_to_string(golden_path).unwrap_or_default();
    assert_eq!(
        expected,
        actual,
        "{} 与 golden 不一致（UPDATE_GOLDEN=1 可重写）\n{}",
        golden_path.display(),
        line_diff(&expected, actual)
    );
}

fn line_diff(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut out = String::new();
    for index in 0..expected.len().max(actual.len()) {
        match (expected.get(index), actual.get(index)) {
            (Some(left), Some(right)) if left == right => {}
            (left, right) => out.push_str(&format!(
                "{:>3} - {}\n{:>3} + {}\n",
                index + 1,
                left.copied().unwrap_or("<missing>"),
                index + 1,
                right.copied().unwrap_or("<missing>")
            )),
        }
    }
    out
}
