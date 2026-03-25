# yatamux Justfile
# https://github.com/casey/just

# Windows では PowerShell を使用
set windows-shell := ["powershell.exe", "-NoProfile", "-Command"]

# デフォルト: ヘルプ表示
default:
    just --list

# デバッグビルドして実行（ログ付き）
run:
    cargo run

# ログ付きデバッグ実行
run-log:
    $env:RUST_LOG="info"; cargo run

# リリースビルド
build:
    cargo build --release

# 全テスト
test:
    cargo test

# cargo install でリリースビルドして ~/.cargo/bin/yatamux.exe にインストール
# --features cli でコンソールサブシステムビルドにすることで
# --help / --version が PowerShell で正常に動作する（Enter 不要）
install:
    cargo install --path . --features cli

# clippy
lint:
    cargo clippy --workspace -- -D warnings

# フォーマット
fmt:
    cargo fmt --all
