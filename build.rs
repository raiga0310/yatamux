// ビルドスクリプト: Windows MSVC リンカーにマニフェストを埋め込む
//
// manifest.xml で以下を設定する:
// - activeCodePage = UTF-8  : プロセス ANSI コードページを UTF-8 にする（CP932 文字化け防止）
// - dpiAwareness = PerMonitorV2: GDI/DWM の DPI 認識モード
//
// Windows 10 1903 (Build 18362) 以降が必要（yatamux の動作要件と同じ）。

fn main() {
    // MSVC ターゲット (x86_64-pc-windows-msvc) のときのみ有効
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        // /MANIFEST:EMBED  : マニフェストを PE バイナリに埋め込む
        // /MANIFESTINPUT:  : 追加で合成するマニフェストファイル
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{manifest_dir}/manifest.xml");
        println!("cargo:rerun-if-changed=manifest.xml");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
