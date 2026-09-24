use notist_editor_node::native::{NodeRuntime, config::Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.is_empty() || arguments.iter().any(|a| a == "--help") {
        println!(
            "notist-editor-node --config PATH [--check]\nRuns signaling and STUN/TURN together, with an optional persistent document peer."
        );
        return Ok(());
    }
    let mut path = None;
    let mut check = false;
    let mut args = arguments.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => path = args.next().map(std::path::PathBuf::from),
            "--check" => check = true,
            _ => anyhow::bail!("unknown argument: {arg}"),
        }
    }
    let config = Config::read(&path.ok_or_else(|| anyhow::anyhow!("--config is required"))?)?;
    if check {
        println!("configuration valid");
        return Ok(());
    }
    let runtime = NodeRuntime::start(config).await?;
    println!(
        "{}",
        serde_json::json!({"event":"ready", "node":runtime.info()})
    );
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {}, _ = runtime.stopped() => { runtime.shutdown().await?; anyhow::bail!("node runtime stopped unexpectedly"); } }
    }
    #[cfg(not(unix))]
    tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = runtime.stopped() => { runtime.shutdown().await?; anyhow::bail!("node runtime stopped unexpectedly"); } }
    runtime.shutdown().await
}
