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

# リリースビルドして %LOCALAPPDATA%\yatamux\yatamux.exe にインストール
install:
    cargo build --release
    New-Item -ItemType Directory -Force -Path "$env:LOCALAPPDATA\yatamux" | Out-Null; Copy-Item -Force "target\release\yatamux.exe" "$env:LOCALAPPDATA\yatamux\yatamux.exe"; Write-Host "Installed to $env:LOCALAPPDATA\yatamux\yatamux.exe"

# インストール先を PATH に追加（PowerShell プロファイルに記述）
add-to-path:
    $dest = "$env:LOCALAPPDATA\yatamux"; if (-not (Test-Path $PROFILE)) { New-Item -Force $PROFILE | Out-Null }; if (-not (Select-String -Quiet -Path $PROFILE -Pattern ([regex]::Escape($dest)))) { Add-Content $PROFILE "`$env:PATH = `"$dest;`$env:PATH`""; Write-Host "Added $dest to PATH in $PROFILE" } else { Write-Host "$dest already in PATH profile" }

# clippy
lint:
    cargo clippy --workspace -- -D warnings

# フォーマット
fmt:
    cargo fmt --all
