use crate::cell::Color;

/// ANSI 16色パレット（xterm 標準）
pub(super) fn ansi16(index: u8) -> Color {
    const P: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0  Black
        (128, 0, 0),     // 1  Red
        (0, 128, 0),     // 2  Green
        (128, 128, 0),   // 3  Yellow
        (0, 0, 128),     // 4  Blue
        (128, 0, 128),   // 5  Magenta
        (0, 128, 128),   // 6  Cyan
        (192, 192, 192), // 7  White
        (128, 128, 128), // 8  Bright Black (Gray)
        (255, 0, 0),     // 9  Bright Red
        (0, 255, 0),     // 10 Bright Green
        (255, 255, 0),   // 11 Bright Yellow
        (0, 0, 255),     // 12 Bright Blue
        (255, 0, 255),   // 13 Bright Magenta
        (0, 255, 255),   // 14 Bright Cyan
        (255, 255, 255), // 15 Bright White
    ];
    let (r, g, b) = P[index as usize % 16];
    Color { r, g, b }
}

/// xterm 256色パレット
pub(super) fn color256(n: u8) -> Color {
    if n < 16 {
        ansi16(n)
    } else if n < 232 {
        let i = n - 16;
        let cube = |v: u8| if v == 0 { 0u8 } else { 55 + 40 * v };
        Color {
            r: cube((i / 36) % 6),
            g: cube((i / 6) % 6),
            b: cube(i % 6),
        }
    } else {
        let v = 8 + 10 * (n - 232);
        Color { r: v, g: v, b: v }
    }
}

/// `38` / `48` に続く拡張色パラメータを解析してインデックスを進める
pub(super) fn parse_extended_color(params: &[u16], i: &mut usize) -> Option<Color> {
    match params.get(*i + 1).copied() {
        Some(5) => {
            let n = params.get(*i + 2).copied()? as u8;
            *i += 2;
            Some(color256(n))
        }
        Some(2) => {
            let r = params.get(*i + 2).copied()? as u8;
            let g = params.get(*i + 3).copied()? as u8;
            let b = params.get(*i + 4).copied()? as u8;
            *i += 4;
            Some(Color { r, g, b })
        }
        _ => None,
    }
}
