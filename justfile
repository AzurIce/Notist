set windows-shell := ["pwsh", "-Command"]

[working-directory: 'docs']
preview:
    cargo run -- preview

stop:
    cargo run -p notist-cli -- daemon stop
