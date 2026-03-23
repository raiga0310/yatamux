//! Windows IME (Input Method Editor) 統合
//!
//! IMM32 API を使った日本語/CJK 入力の実装。
//! WezTerm の実証済みパターンに従い、TSF より単純な IMM32 を採用。
//!
//! ## ライフサイクル
//! ```text
//! WM_IME_STARTCOMPOSITION  → composing = true
//! WM_IME_COMPOSITION       → preedit 文字列を更新、候補ウィンドウを配置
//!   GCS_COMPSTR            →   下線付きプリエディット文字列を取得
//!   GCS_COMPATTR           →   変換状態属性（下線スタイル判定に使用）
//!   GCS_RESULTSTR          →   確定文字列を取得し PTY に送信
//! WM_IME_ENDCOMPOSITION    → composing = false、プリエディットをクリア
//! ```
//!
//! ## 候補ウィンドウの配置
//! `ImmSetCompositionWindow` と `ImmSetCandidateWindow` で
//! 現在のカーソルセル座標（ピクセル変換後）を IME に通知する。

use std::sync::{Arc, Mutex};

/// プリエディット文字の属性（下線スタイルに使用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreeditAttr {
    /// 未変換入力（点線下線）
    Input,
    /// 変換済み・確定前（実線下線）
    Converted,
    /// 現在のターゲット変換（太実線下線）
    TargetConverted,
    /// ターゲット未変換（太点線下線）
    TargetNotConverted,
}

/// プリエディット文字列のセグメント
#[derive(Debug, Clone)]
pub struct PreeditSegment {
    pub text: String,
    pub attr: PreeditAttr,
}

/// IME の現在状態（レンダラーと共有）
#[derive(Debug, Clone, Default)]
pub struct ImeState {
    /// 変換中か
    pub composing: bool,
    /// プリエディット文字列（セグメント分割済み）
    pub preedit: Vec<PreeditSegment>,
    /// プリエディット内のカーソル位置（文字インデックス）
    pub preedit_cursor: usize,
    /// 確定済み文字列（PTY に送信後クリア）
    pub committed: Option<String>,
}

impl ImeState {
    /// プリエディット全体のテキストを結合して返す
    pub fn preedit_text(&self) -> String {
        self.preedit.iter().map(|s| s.text.as_str()).collect()
    }

    /// プリエディットが空か
    pub fn is_empty(&self) -> bool {
        !self.composing || self.preedit.is_empty()
    }
}

/// セルのピクセル座標（候補ウィンドウ配置用）
#[derive(Debug, Clone, Copy)]
pub struct CellPixelPos {
    pub x: i32,
    pub y: i32,
    pub cell_width: i32,
    pub cell_height: i32,
}

/// IME ハンドラ（Win32 IMM32 ラッパー）
///
/// `Arc<Mutex<ImeState>>` を介してレンダラースレッドと状態を共有する。
pub struct ImeHandler {
    pub state: Arc<Mutex<ImeState>>,
}

impl Default for ImeHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ImeHandler {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ImeState::default())),
        }
    }

    /// WM_IME_STARTCOMPOSITION を処理する
    pub fn on_start_composition(&self) {
        let mut state = self.state.lock().unwrap();
        state.composing = true;
        state.preedit.clear();
        state.preedit_cursor = 0;
        state.committed = None;
    }

    /// WM_IME_ENDCOMPOSITION を処理する
    pub fn on_end_composition(&self) {
        let mut state = self.state.lock().unwrap();
        state.composing = false;
        state.preedit.clear();
        state.preedit_cursor = 0;
    }

    /// WM_IME_COMPOSITION を処理する
    ///
    /// `lparam` は変更内容を示すビットフラグ。
    /// Windows: hwnd は実際の HWND を渡す（非 Windows ではダミー値で可）。
    #[cfg(windows)]
    pub fn on_composition(&self, hwnd: windows::Win32::Foundation::HWND, lparam: usize) {
        use windows::Win32::UI::Input::Ime::{
            ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
            GCS_COMPSTR, GCS_CURSORPOS, GCS_RESULTSTR,
        };

        unsafe {
            let himc = ImmGetContext(hwnd);
            if himc.is_invalid() {
                return;
            }

            let lparam = lparam as u32;

            // ── 確定文字列 (GCS_RESULTSTR) ──────────────────────────────
            if lparam & GCS_RESULTSTR.0 != 0 {
                if let Some(result) = get_composition_string_w(himc, GCS_RESULTSTR) {
                    let mut state = self.state.lock().unwrap();
                    state.committed = Some(result);
                    state.preedit.clear();
                }
            }

            // ── プリエディット文字列 (GCS_COMPSTR) ────────────────────
            if lparam & GCS_COMPSTR.0 != 0 {
                let comp_str = get_composition_string_w(himc, GCS_COMPSTR)
                    .unwrap_or_default();

                // 属性バイト列（各文字に対応）
                let attrs = get_composition_attrs(himc);

                // カーソル位置
                let cursor = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0) as usize;

                let segments = build_preedit_segments(&comp_str, &attrs);

                let mut state = self.state.lock().unwrap();
                state.preedit = segments;
                state.preedit_cursor = cursor;
            }

            let _ = ImmReleaseContext(hwnd, himc);
        }
    }

    /// 候補ウィンドウをカーソル位置に配置する
    ///
    /// `cursor_pixel` はターミナルグリッドのカーソルセルのピクセル座標。
    /// ウィンドウのスクロールやパディングを考慮した絶対座標を渡すこと。
    #[cfg(windows)]
    pub fn update_candidate_window(
        &self,
        hwnd: windows::Win32::Foundation::HWND,
        cursor_pixel: CellPixelPos,
    ) {
        use windows::Win32::Foundation::{POINT, RECT};
        use windows::Win32::UI::Input::Ime::{
            CFS_EXCLUDE, CFS_POINT, CANDIDATEFORM, COMPOSITIONFORM,
            ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
            ImmSetCompositionWindow,
        };

        unsafe {
            let himc = ImmGetContext(hwnd);
            if himc.is_invalid() {
                return;
            }

            let pt = POINT {
                x: cursor_pixel.x,
                y: cursor_pixel.y,
            };

            // CompositionForm: プリエディット文字列の描画基点
            let comp_form = COMPOSITIONFORM {
                dwStyle: CFS_POINT,
                ptCurrentPos: pt,
                rcArea: RECT::default(),
            };
            let _ = ImmSetCompositionWindow(himc, &comp_form);

            // CandidateForm: 候補ウィンドウの位置
            // CFS_EXCLUDE でカーソル行を除外エリアに指定し、候補が文字を隠さないようにする
            let cand_form = CANDIDATEFORM {
                dwIndex: 0,
                dwStyle: CFS_EXCLUDE,
                ptCurrentPos: pt,
                rcArea: RECT {
                    left: cursor_pixel.x,
                    top: cursor_pixel.y,
                    right: cursor_pixel.x + cursor_pixel.cell_width,
                    bottom: cursor_pixel.y + cursor_pixel.cell_height,
                },
            };
            let _ = ImmSetCandidateWindow(himc, &cand_form);

            let _ = ImmReleaseContext(hwnd, himc);
        }
    }

    /// Windows 以外のプラットフォーム用スタブ
    #[cfg(not(windows))]
    pub fn on_composition(&self, _hwnd: usize, _lparam: usize) {}

    #[cfg(not(windows))]
    pub fn update_candidate_window(&self, _hwnd: usize, _cursor_pixel: CellPixelPos) {}
}

// ── 内部ヘルパー ────────────────────────────────────────────────────────────

/// `ImmGetCompositionStringW` で UTF-16 文字列を取得し UTF-8 に変換する
#[cfg(windows)]
unsafe fn get_composition_string_w(
    himc: windows::Win32::UI::Input::Ime::HIMC,
    index: windows::Win32::UI::Input::Ime::IME_COMPOSITION_STRING,
) -> Option<String> {
    use windows::Win32::UI::Input::Ime::ImmGetCompositionStringW;

    // バッファサイズを取得（バイト数）
    let byte_len = ImmGetCompositionStringW(himc, index, None, 0);
    if byte_len <= 0 {
        return None;
    }

    // UTF-16 LE バッファを確保
    let u16_len = byte_len as usize / 2;
    let mut buf: Vec<u16> = vec![0u16; u16_len];

    let written = ImmGetCompositionStringW(
        himc,
        index,
        Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
        byte_len as u32,
    );
    if written <= 0 {
        return None;
    }

    String::from_utf16(&buf[..written as usize / 2]).ok()
}

/// `GCS_COMPATTR` で属性バイト列を取得する
#[cfg(windows)]
unsafe fn get_composition_attrs(
    himc: windows::Win32::UI::Input::Ime::HIMC,
) -> Vec<u8> {
    use windows::Win32::UI::Input::Ime::{GCS_COMPATTR, ImmGetCompositionStringW};

    let byte_len = ImmGetCompositionStringW(himc, GCS_COMPATTR, None, 0);
    if byte_len <= 0 {
        return Vec::new();
    }

    let mut buf: Vec<u8> = vec![0u8; byte_len as usize];
    ImmGetCompositionStringW(
        himc,
        GCS_COMPATTR,
        Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
        byte_len as u32,
    );
    buf
}

/// 文字列と属性バイト列からプリエディットセグメントを構築する
///
/// 連続する同一属性の文字をまとめてセグメントにする。
fn build_preedit_segments(text: &str, attrs: &[u8]) -> Vec<PreeditSegment> {
    #[cfg(windows)]
    use windows::Win32::UI::Input::Ime::{
        ATTR_CONVERTED, ATTR_TARGET_CONVERTED, ATTR_TARGET_NOTCONVERTED,
    };

    if text.is_empty() {
        return Vec::new();
    }

    // 属性が空またはサイズ不一致の場合は全体を ATTR_INPUT として扱う
    if attrs.is_empty() {
        return vec![PreeditSegment {
            text: text.to_string(),
            attr: PreeditAttr::Input,
        }];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut segments: Vec<PreeditSegment> = Vec::new();
    let mut current_text = String::new();
    let mut current_attr: Option<PreeditAttr> = None;

    for (i, &ch) in chars.iter().enumerate() {
        let raw_attr = attrs.get(i).copied().unwrap_or(0);

        #[cfg(windows)]
        let attr = match raw_attr as u32 {
            x if x == ATTR_TARGET_CONVERTED => PreeditAttr::TargetConverted,
            x if x == ATTR_CONVERTED => PreeditAttr::Converted,
            x if x == ATTR_TARGET_NOTCONVERTED => PreeditAttr::TargetNotConverted,
            _ => PreeditAttr::Input,
        };

        #[cfg(not(windows))]
        let attr = PreeditAttr::Input;

        if current_attr == Some(attr) {
            current_text.push(ch);
        } else {
            if let Some(a) = current_attr {
                segments.push(PreeditSegment { text: current_text.clone(), attr: a });
                current_text.clear();
            }
            current_text.push(ch);
            current_attr = Some(attr);
        }
    }

    if let Some(a) = current_attr {
        if !current_text.is_empty() {
            segments.push(PreeditSegment { text: current_text, attr: a });
        }
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ime_state_default() {
        let state = ImeState::default();
        assert!(!state.composing);
        assert!(state.preedit.is_empty());
        assert!(state.committed.is_none());
    }

    #[test]
    fn test_start_end_composition() {
        let handler = ImeHandler::new();
        handler.on_start_composition();
        {
            let state = handler.state.lock().unwrap();
            assert!(state.composing);
        }
        handler.on_end_composition();
        {
            let state = handler.state.lock().unwrap();
            assert!(!state.composing);
        }
    }

    #[test]
    fn test_build_preedit_segments_no_attrs() {
        let segments = build_preedit_segments("こんにちは", &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "こんにちは");
        assert_eq!(segments[0].attr, PreeditAttr::Input);
    }

    #[test]
    fn test_build_preedit_segments_with_attrs() {
        // 全文字を TargetConverted (1) として扱う
        #[cfg(windows)]
        {
            let text = "日本語";
            let attrs = vec![
                windows::Win32::UI::Input::Ime::ATTR_TARGET_CONVERTED as u8,
                windows::Win32::UI::Input::Ime::ATTR_TARGET_CONVERTED as u8,
                windows::Win32::UI::Input::Ime::ATTR_INPUT as u8,
            ];
            let segments = build_preedit_segments(text, &attrs);
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0].attr, PreeditAttr::TargetConverted);
            assert_eq!(segments[1].attr, PreeditAttr::Input);
        }
    }

    #[test]
    fn test_preedit_text() {
        let state = ImeState {
            composing: true,
            preedit: vec![
                PreeditSegment { text: "日本".to_string(), attr: PreeditAttr::TargetConverted },
                PreeditSegment { text: "語".to_string(), attr: PreeditAttr::Input },
            ],
            preedit_cursor: 2,
            committed: None,
        };
        assert_eq!(state.preedit_text(), "日本語");
    }

    // E-1: WM_IME_STARTCOMPOSITION で composing=true、各フィールドがクリアされる
    #[test]
    fn test_start_composition_initializes_state() {
        let handler = ImeHandler::new();
        // 事前に committed と preedit をセット
        {
            let mut s = handler.state.lock().unwrap();
            s.committed = Some("既存テキスト".to_string());
            s.preedit = vec![PreeditSegment {
                text: "あ".to_string(),
                attr: PreeditAttr::Input,
            }];
            s.preedit_cursor = 1;
        }
        handler.on_start_composition();
        let s = handler.state.lock().unwrap();
        assert!(s.composing, "composing should be true");
        assert!(s.preedit.is_empty(), "preedit should be cleared");
        assert_eq!(s.preedit_cursor, 0, "cursor should be reset");
        assert!(s.committed.is_none(), "committed should be cleared");
    }

    // E-2: プリエディット文字列が複数セグメントに分割される
    #[test]
    fn test_preedit_multi_segment_concat() {
        let state = ImeState {
            composing: true,
            preedit: vec![
                PreeditSegment { text: "にほ".to_string(), attr: PreeditAttr::Input },
                PreeditSegment { text: "ん".to_string(), attr: PreeditAttr::TargetConverted },
                PreeditSegment { text: "ご".to_string(), attr: PreeditAttr::Converted },
            ],
            preedit_cursor: 2,
            committed: None,
        };
        assert_eq!(state.preedit_text(), "にほんご");
    }

    // E-3: WM_IME_ENDCOMPOSITION で composing=false、プリエディットがクリア
    #[test]
    fn test_end_composition_clears_preedit() {
        let handler = ImeHandler::new();
        handler.on_start_composition();
        {
            let mut s = handler.state.lock().unwrap();
            s.preedit = vec![PreeditSegment {
                text: "かんじ".to_string(),
                attr: PreeditAttr::Input,
            }];
        }
        handler.on_end_composition();
        let s = handler.state.lock().unwrap();
        assert!(!s.composing, "composing should be false after end");
        assert!(s.preedit.is_empty(), "preedit should be cleared");
    }

    // E-4: confirmed テキストが読み取り可能で、送信後クリアできる
    #[test]
    fn test_committed_text_read_and_clear() {
        let handler = ImeHandler::new();
        handler.on_start_composition();
        {
            let mut s = handler.state.lock().unwrap();
            s.committed = Some("日本語".to_string());
        }
        // window.rs のロジックをシミュレート: committed を取り出し → 送信 → クリア
        let committed = handler.state.lock().unwrap().committed.clone();
        assert_eq!(committed, Some("日本語".to_string()));
        handler.state.lock().unwrap().committed = None;
        assert!(handler.state.lock().unwrap().committed.is_none());
    }

    // E-5: composing=true のとき WM_CHAR 入力をスキップすべき条件が成立する
    #[test]
    fn test_composing_flag_should_suppress_wm_char() {
        let handler = ImeHandler::new();
        assert!(!handler.state.lock().unwrap().composing, "初期値は false");
        handler.on_start_composition();
        assert!(
            handler.state.lock().unwrap().composing,
            "on_start_composition 後は true — WM_CHAR は skip されるべき"
        );
        handler.on_end_composition();
        assert!(!handler.state.lock().unwrap().composing, "on_end_composition 後は false");
    }

    // is_empty: composing=false なら常に empty
    #[test]
    fn test_is_empty_when_not_composing() {
        let state = ImeState::default();
        assert!(state.is_empty());
    }

    // is_empty: composing=true かつ preedit が空でも empty
    #[test]
    fn test_is_empty_when_composing_but_no_preedit() {
        let state = ImeState {
            composing: true,
            preedit: vec![],
            preedit_cursor: 0,
            committed: None,
        };
        assert!(state.is_empty());
    }

    // is_empty: composing=true かつ preedit あり → not empty
    #[test]
    fn test_is_not_empty_when_composing_with_preedit() {
        let state = ImeState {
            composing: true,
            preedit: vec![PreeditSegment {
                text: "あ".to_string(),
                attr: PreeditAttr::Input,
            }],
            preedit_cursor: 1,
            committed: None,
        };
        assert!(!state.is_empty());
    }

    // build_preedit_segments: 属性なしで全体が Input セグメント 1 つ
    #[test]
    fn test_build_segments_empty_attrs_single_segment() {
        let segs = build_preedit_segments("てすと", &[]);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "てすと");
        assert_eq!(segs[0].attr, PreeditAttr::Input);
    }

    // build_preedit_segments: 空テキストは空ベクタ
    #[test]
    fn test_build_segments_empty_text() {
        let segs = build_preedit_segments("", &[1, 2, 3]);
        assert!(segs.is_empty());
    }
}
