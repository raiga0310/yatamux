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
    $dest = "$env:LOCALAPPDATA\yatamux"; `
    New-Item -ItemType Directory -Force -Path $dest | Out-Null; `
    Copy-Item -Force target\release\yatamux.exe "$dest\yatamux.exe"; `
    Write-Host "Installed to $dest\yatamux.exe"; `
    if ($env:PATH -notlike "*$dest*") { Write-Host "Tip: run 'just add-to-path' to add it to PATH" }

# インストール先を PATH に追加（PowerShell プロファイルに記述）
add-to-path:
    $dest = "$env:LOCALAPPDATA\yatamux"; `
    $line = "`$env:PATH = `"$dest;`$env:PATH`""; `
    if (-not (Test-Path $PROFILE)) { New-Item -Force $PROFILE | Out-Null }; `
    if (-not (Get-Content $PROFILE -Raw | Select-String -Quiet ([regex]::Escape($dest)))) { `
        Add-Content -Path $PROFILE -Value $line; `
        Write-Host "Added $dest to PATH in $PROFILE" `
    } else { Write-Host "$dest is already in PATH profile" }

# clippy
lint:
    cargo clippy --workspace -- -D warnings

# フォーマット
fmt:
    cargo fmt --all
