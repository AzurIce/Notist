//! 手动对拍工具。
//!
//! - `cargo run -p notist-ir -- <file.not>`：dump 一个源文件的产物；
//! - `cargo run -p notist-ir -- --package <dir>`：dump 一个包的接口与实现解析结果。

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--workspace") => {
            let Some(directory) = arguments.next() else {
                eprintln!("usage: notist-ir --workspace <dir>");
                return ExitCode::FAILURE;
            };
            match notist_ir::package::load_graph(Path::new(&directory)) {
                Ok(graph) => {
                    print!("{}", notist_ir::dump::dump_graph(&graph));
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("--package") => {
            let Some(directory) = arguments.next() else {
                eprintln!("usage: notist-ir --package <dir>");
                return ExitCode::FAILURE;
            };
            match notist_ir::package::load(Path::new(&directory)) {
                Ok(report) => {
                    print!("{}", notist_ir::dump::dump_package(&report));
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(path) => {
            let source = match std::fs::read_to_string(path) {
                Ok(source) => source,
                Err(error) => {
                    eprintln!("{path}: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let result = if path.ends_with(".notc") {
                notist_ir::compile_code(&source)
            } else {
                notist_ir::compile(&source)
            };
            print!("{}", notist_ir::dump::dump(&result));
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("usage: notist-ir <file.not> | notist-ir --package <dir>");
            ExitCode::FAILURE
        }
    }
}
