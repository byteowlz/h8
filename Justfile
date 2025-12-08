set shell := ["bash", "-cu"]

install:
    uv sync
    cargo install --path h8 --locked

service-start:
    cargo run --manifest-path h8/Cargo.toml -- service start

service-stop:
    cargo run --manifest-path h8/Cargo.toml -- service stop

service-status:
    cargo run --manifest-path h8/Cargo.toml -- service status

cli *args:
    cargo run --manifest-path h8/Cargo.toml -- {{args}}
