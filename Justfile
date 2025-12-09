set shell := ["bash", "-cu"]

install:
    uv sync
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
