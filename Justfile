set shell := ["bash", "-cu"]

install:
    uv sync
    uv tool install -e . --force
    cargo install --path h8-client/h8-cli --locked
    cargo install --path h8-client/h8-tui --locked

service-start:
    cargo run --manifest-path h8-client/Cargo.toml -- service start

service-stop:
    cargo run --manifest-path h8-client/Cargo.toml -- service stop

service-status:
    cargo run --manifest-path h8-client/Cargo.toml -- service status

cli *args:
    cargo run --manifest-path h8-client/Cargo.toml -- {{args}}

# Bump version: just bump [major|minor|patch]
bump level:
    cd h8-client && cargo release {{level}} --no-publish --no-push --no-tag --execute --no-confirm
    uv version --bump {{level}}
