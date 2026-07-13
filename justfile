set windows-shell := ["pwsh", "-Command"]

[working-directory: 'docs']
preview:
    cargo run -- preview
