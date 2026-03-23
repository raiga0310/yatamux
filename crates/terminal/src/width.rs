//! CJK 文字幅計算
//!
//! 要件定義書 §4 に基づく権威的幅テーブル。
//! ConPTY のカーソル位置は信用せず、このモジュールの計算を優先する。

use unicode_width::UnicodeWidthChar;

/// 曖昧幅文字（East Asian Ambiguous）の扱い設定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbiguousWidth {
    /// 1 セル（西洋コンテキスト、デフォルト）
    #[default]
    Narrow,
    /// 2 セル（東アジアコンテキスト）
    Wide,
}

/// CJK 幅計算の設定
#[derive(Debug, Clone, Default)]
pub struct CjkWidthConfig {
    pub ambiguous: AmbiguousWidth,
    /// コードポイント範囲ごとの幅オーバーライド: (start, end_inclusive, width)
    pub overrides: Vec<(u32, u32, u8)>,
}

impl CjkWidthConfig {
    /// 文字の表示幅を返す（1 または 2）
    ///
    /// 制御文字・結合文字は 0 を返す。
    /// 全角文字は 2、その他は 1 を基本とする。
    pub fn char_width(&self, c: char) -> u8 {
        let cp = c as u32;

        // 範囲オーバーライドを先に確認
        for &(start, end, width) in &self.overrides {
            if cp >= start && cp <= end {
                return width;
            }
        }

        // 半角カタカナ濁点・半濁点は端末で独立 1 セルを占有する
        // （unicode_width は combining mark として 0 を返すため特別処理が必要）
        if cp == 0xFF9E || cp == 0xFF9F {
            return 1;
        }

        // 曖昧幅文字の特別処理
        if is_east_asian_ambiguous(c) {
            return match self.ambiguous {
                AmbiguousWidth::Narrow => 1,
                AmbiguousWidth::Wide => 2,
            };
        }

        // unicode-width クレートによる標準計算
        match UnicodeWidthChar::width(c) {
            Some(0) => 0,
            Some(2) => 2,
            _ => 1,
        }
    }

    /// 文字列の表示幅を返す（書記素クラスタ単位）
    ///
    /// - VS16 (U+FE0F) を含むグラフィームは絵文字表示とみなし 2 セル
    /// - それ以外はグラフィーム内の全コードポイント幅を合算
    ///   （半角濁音 U+FF9E 等の間隔付き結合マークに対応）
    pub fn str_width(&self, s: &str) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        s.graphemes(true)
            .map(|g| {
                let chars: Vec<char> = g.chars().collect();
                // VS16 絵文字表示セレクタが含まれる → 2 セル幅
                if chars.contains(&'\u{FE0F}') {
                    return 2;
                }
                // グラフィーム内の全コードポイント幅を合算
                chars.iter().map(|c| self.char_width(*c) as usize).sum()
            })
            .sum()
    }
}

/// East Asian Ambiguous 文字の判定
///
/// UAX #11 の Ambiguous カテゴリに属する代表的な範囲。
/// 完全なリストは Unicode データベースを参照のこと。
fn is_east_asian_ambiguous(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        // ラテン拡張
        0x00A1 | 0x00A4 | 0x00A7..=0x00A8 | 0x00AA | 0x00AD..=0x00AE |
        0x00B0..=0x00B4 | 0x00B6..=0x00BA | 0x00BC..=0x00BF |
        // ギリシャ文字
        0x0391..=0x03C9 |
        // 罫線素片
        0x2500..=0x257F |
        // ブロック要素
        0x2580..=0x259F |
        // 幾何学模様
        0x25A0..=0x25FF |
        // その他記号
        0x2600..=0x26FF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cjk_unified_ideograph_is_wide() {
        let cfg = CjkWidthConfig::default();
        // 漢字（全角 2 セル）
        assert_eq!(cfg.char_width('漢'), 2);
        assert_eq!(cfg.char_width('字'), 2);
    }

    #[test]
    fn test_ascii_is_narrow() {
        let cfg = CjkWidthConfig::default();
        assert_eq!(cfg.char_width('A'), 1);
        assert_eq!(cfg.char_width('z'), 1);
    }

    #[test]
    fn test_full_width_katakana_is_wide() {
        let cfg = CjkWidthConfig::default();
        // 全角カタカナ U+30A2 ア
        assert_eq!(cfg.char_width('ア'), 2);
    }

    #[test]
    fn test_half_width_katakana_is_narrow() {
        let cfg = CjkWidthConfig::default();
        // 半角カタカナ U+FF71 ｱ
        assert_eq!(cfg.char_width('ｱ'), 1);
    }

    #[test]
    fn test_ambiguous_width_config() {
        let narrow_cfg = CjkWidthConfig { ambiguous: AmbiguousWidth::Narrow, overrides: vec![] };
        let wide_cfg = CjkWidthConfig { ambiguous: AmbiguousWidth::Wide, overrides: vec![] };

        // ─ (U+2500) は Ambiguous
        assert_eq!(narrow_cfg.char_width('─'), 1);
        assert_eq!(wide_cfg.char_width('─'), 2);
    }

    #[test]
    fn test_width_override() {
        let cfg = CjkWidthConfig {
            ambiguous: AmbiguousWidth::Narrow,
            // ギリシャ文字を強制的に 2 セル
            overrides: vec![(0x0391, 0x03C9, 2)],
        };
        assert_eq!(cfg.char_width('α'), 2);
    }

    // C-8: 韓国語 Hangul 音節 = 2 セル
    #[test]
    fn test_hangul_syllable_is_wide() {
        let cfg = CjkWidthConfig::default();
        assert_eq!(cfg.char_width('안'), 2); // U+C548
        assert_eq!(cfg.char_width('녕'), 2); // U+B155
        assert_eq!(cfg.char_width('하'), 2); // U+D558
    }

    // C-4: 半角濁音 ｶﾞ = 2 コードポイント × 各 1 セル = 合計 2 セル
    #[test]
    fn test_half_width_voiced_katakana_str_width() {
        let cfg = CjkWidthConfig::default();
        // U+FF76 (ｶ) + U+FF9E (ﾞ) — それぞれ半角 1 セル = 合計 2 セル
        assert_eq!(cfg.str_width("ｶﾞ"), 2);
    }

    // C-10: 結合文字（combining grave accent U+0300）は幅 0
    #[test]
    fn test_combining_char_is_zero_width() {
        let cfg = CjkWidthConfig::default();
        assert_eq!(cfg.char_width('\u{0300}'), 0); // combining grave accent
    }

    // C-10: 結合文字付きグラフィームクラスタの str_width は基底文字の幅のみ
    #[test]
    fn test_str_width_with_combining_char() {
        let cfg = CjkWidthConfig::default();
        // 'a' (1) + combining grave (0) = 1 セル
        assert_eq!(cfg.str_width("a\u{0300}"), 1);
    }

    // C-11: ゼロ幅文字（ZERO WIDTH SPACE U+200B）= 幅 0
    #[test]
    fn test_zero_width_space() {
        let cfg = CjkWidthConfig::default();
        assert_eq!(cfg.char_width('\u{200B}'), 0);
    }

    // C-11: ZWJ (ZERO WIDTH JOINER U+200D) = 幅 0
    #[test]
    fn test_zero_width_joiner() {
        let cfg = CjkWidthConfig::default();
        assert_eq!(cfg.char_width('\u{200D}'), 0);
    }

    // C-7: VS16 (U+FE0F) で narrow → wide (絵文字表示 = 2 セル)
    #[test]
    fn test_vs16_makes_char_wide() {
        let cfg = CjkWidthConfig::default();
        // ♀ (U+2640, Ambiguous=1) + VS16 (U+FE0F) → 絵文字表示 = 2 セル
        assert_eq!(cfg.str_width("♀\u{FE0F}"), 2);
    }

    // C-9: NFD 韓国語 → NFC 変換
    #[test]
    fn test_nfc_normalization_korean() {
        use crate::grid::normalize_nfc;
        // NFD: U+110B (ㅇ) + U+1161 (ㅏ) → NFC: U+C544 (아)
        let nfd = "\u{110B}\u{1161}";
        let nfc = normalize_nfc(nfd);
        assert_eq!(nfc, "아");
    }
}
