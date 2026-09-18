use notist_next::{Runtime, package, snapshot};
use std::{fs, path::Path, process::ExitCode};

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(path) = args.first() else {
        return Err(
            "usage: notist-next PACKAGE|FILE.not|FILE.notc [--html|--snapshot|--bundle DIR]".into(),
        );
    };
    if path == "--help" {
        println!("notist-next PACKAGE|FILE.not|FILE.notc [--html|--snapshot|--bundle DIR]");
        return Ok(());
    }
    let path = Path::new(path).canonicalize().map_err(|e| e.to_string())?;
    let mut loaded = if path.is_dir() {
        package::load(&path)?
    } else {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid filename")?
            .to_owned();
        let mut runtime = Runtime::default();
        runtime.sources.insert(
            name.clone(),
            fs::read_to_string(&path).map_err(|e| e.to_string())?,
        );
        package::Loaded {
            runtime,
            entry: name,
            components: Default::default(),
            resources: Default::default(),
        }
    };
    let mode = args.get(1).map(String::as_str).unwrap_or("");
    if mode == "--request" {
        println!("{}", loaded.request());
        return Ok(());
    }
    if mode == "--snapshot" {
        let request = loaded.request();
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot::analyze(&request.to_string())).unwrap()
        );
        return Ok(());
    }
    let result = loaded.runtime.evaluate(&loaded.entry);
    for warning in &result.warnings {
        eprintln!("{warning}");
    }
    match mode {
        "" => println!(
            "{}",
            serde_json::to_string_pretty(&result.content.to_json()).unwrap()
        ),
        "--html" => println!("{}", result.content.html()),
        "--bundle" => {
            let dir = Path::new(args.get(2).ok_or("--bundle requires output directory")?);
            if dir.exists() {
                return Err("bundle destination must not exist".into());
            }
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            for (name, bytes) in loaded.resources {
                let p = dir.join(name);
                fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
                fs::write(p, bytes).map_err(|e| e.to_string())?;
            }
            let manifest = serde_json::to_string(&loaded.components).unwrap();
            fs::write(dir.join("components.json"), manifest).map_err(|e| e.to_string())?;
            fs::write(
                dir.join("content.json"),
                result.content.to_json().to_string(),
            )
            .map_err(|e| e.to_string())?;
            fs::write(dir.join("renderer.js"), include_str!("../web/renderer.js"))
                .map_err(|e| e.to_string())?;
            fs::write(dir.join("index.html"),"<!doctype html><meta charset=\"utf-8\"><title>Notist</title><main id=\"document\"></main><script type=\"module\">import {mount} from './renderer.js'; const [content,components]=await Promise.all(['content.json','components.json'].map(p=>fetch(p).then(r=>r.json()))); await mount(document.querySelector('main'),content,components);</script>").map_err(|e|e.to_string())?;
        }
        _ => return Err("unknown output option".into()),
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
