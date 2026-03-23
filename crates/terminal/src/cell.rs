//! ターミナルセルの表現
//!
//! CJK 全角文字はリーディングセル（width=2）＋トレーリングセル（Continuation）
//! のペアで表現する。Windows Terminal の DbcsAttribute に相当。

use serde::{Deserialize, Serialize};

/// RGB カラー
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0 };
    pub const WHITE: Self = Self { r: 255, g: 255, b: 255 };
}

/// セルのスタイル属性
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub blink: bool,
    pub reverse: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

/// セルの内容種別
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellContent {
    /// 通常の書記素クラスタ（1 または 2 セル幅）
    Grapheme {
        /// 書記素クラスタ文字列（結合文字含む）
        text: String,
        /// 表示幅（1 = 半角、2 = 全角）
        width: u8,
    },
    /// 全角文字の右半分（Continuation/Trailing half）
    ///
    /// 全角文字はカラム N に Grapheme{width:2}、カラム N+1 に Continuation を配置。
    /// レンダリング時は Continuation を描画しない。
    Continuation,
    /// 空セル（スペース相当）
    #[default]
    Blank,
}

/// ターミナルグリッドの 1 セル
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cell {
    pub content: CellContent,
    pub style: CellStyle,
}

impl Cell {
    pub fn blank() -> Self {
        Self::default()
    }

    pub fn from_char(c: char, width: u8, style: CellStyle) -> Self {
        Self {
            content: CellContent::Grapheme {
                text: c.to_string(),
                width,
            },
            style,
        }
    }

    pub fn from_grapheme(text: String, width: u8, style: CellStyle) -> Self {
        Self {
            content: CellContent::Grapheme { text, width },
            style,
        }
    }

    pub fn continuation(style: CellStyle) -> Self {
        Self {
            content: CellContent::Continuation,
            style,
        }
    }

    /// セルが実際に描画される文字を持つか
    pub fn is_drawable(&self) -> bool {
        matches!(&self.content, CellContent::Grapheme { .. })
    }

    /// セルの表示幅（Continuation は 0）
    pub fn display_width(&self) -> u8 {
        match &self.content {
            CellContent::Grapheme { width, .. } => *width,
            CellContent::Continuation => 0,
            CellContent::Blank => 1,
        }
    }
}
