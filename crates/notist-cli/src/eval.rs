use notist_analysis::{EvaluationSession as Runtime, package};
use notist_html::RenderHtml;
use notist_next::snapshot;
use std::{fs, path::PathBuf};

#[derive(clap::Args)]
pub struct Args {
    /// Package directory or .not / .notc source file.
    pub path: PathBuf,
    /// Emit HTML.
    #[arg(long, group = "output")]
    pub html: bool,
    /// Emit the analysis snapshot as JSON.
    #[arg(long, group = "output")]
    pub snapshot: bool,
    /// Emit the portable analysis request as JSON.
    #[arg(long, group = "output")]
    pub request: bool,
    /// Write a browser bundle to a new directory.
    #[arg(long, value_name = "DIR", group = "output")]
    pub bundle: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<(), String> {
    let path = args.path.canonicalize().map_err(|e| e.to_string())?;
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
            paths: std::collections::BTreeMap::from([(name.clone(), path.clone())]),
            runtime,
            entry: name,
            components: Default::default(),
            resources: Default::default(),
        }
    };
    if args.request {
        println!("{}", loaded.request());
        return Ok(());
    }
    if args.snapshot {
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
    if let Some(dir) = args.bundle {
        if dir.exists() {
            return Err("bundle destination must not exist".into());
        }
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, bytes) in loaded.resources {
            let p = dir.join(name);
            fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::write(p, bytes).map_err(|e| e.to_string())?;
        }
        let manifest = serde_json::to_string(&loaded.components).unwrap();
        fs::write(dir.join("components.json"), manifest).map_err(|e| e.to_string())?;
        let attributes = result
            .attributes
            .iter()
            .map(|(k, v)| (k, v.to_json()))
            .collect::<std::collections::BTreeMap<_, _>>();
        fs::write(
            dir.join("attributes.json"),
            serde_json::to_vec(&attributes).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        fs::write(
            dir.join("content.json"),
            result.content.to_json().to_string(),
        )
        .map_err(|e| e.to_string())?;
        fs::write(dir.join("renderer.js"), notist_html::RENDERER_JS).map_err(|e| e.to_string())?;
        fs::write(dir.join("index.html"), notist_html::BUNDLE_HTML).map_err(|e| e.to_string())?;
    } else if args.html {
        println!("{}", result.content.html());
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&result.content.to_json()).unwrap()
        );
    }
    Ok(())
}
