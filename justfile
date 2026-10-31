build:
    cargo build --release

lint:
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets -- -D warnings

test:
    cargo test --locked --workspace --all-targets

readme:
    cargo xtask readme

readme-check:
    cargo --locked xtask readme --check

preview:
    scripts/preview.sh
