use crate::cell::CellStyle;

use super::color::{ansi16, parse_extended_color};

pub(super) fn apply_sgr(current_style: &mut CellStyle, params: &[u16]) {
    let mut i = 0;
    while i < params.len() {
        match params[i] {
            0 => *current_style = CellStyle::default(),
            1 => current_style.bold = true,
            3 => current_style.italic = true,
            4 => current_style.underline = true,
            5 => current_style.blink = true,
            7 => current_style.reverse = true,
            9 => current_style.strikethrough = true,
            22 => current_style.bold = false,
            23 => current_style.italic = false,
            24 => current_style.underline = false,
            25 => current_style.blink = false,
            27 => current_style.reverse = false,
            29 => current_style.strikethrough = false,
            30..=37 => current_style.fg = Some(ansi16(params[i] as u8 - 30)),
            39 => current_style.fg = None,
            40..=47 => current_style.bg = Some(ansi16(params[i] as u8 - 40)),
            49 => current_style.bg = None,
            90..=97 => current_style.fg = Some(ansi16(params[i] as u8 - 90 + 8)),
            100..=107 => current_style.bg = Some(ansi16(params[i] as u8 - 100 + 8)),
            38 => {
                if let Some(c) = parse_extended_color(params, &mut i) {
                    current_style.fg = Some(c);
                }
            }
            48 => {
                if let Some(c) = parse_extended_color(params, &mut i) {
                    current_style.bg = Some(c);
                }
            }
            _ => {}
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use crate::cell::Color;
    use crate::grid::Grid;
    use crate::vt::{feed_bytes, VtProcessor};
    use crate::width::CjkWidthConfig;
    use vte::Parser;

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    fn feed(grid: &mut Grid, data: &[u8]) {
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(grid);
        feed_bytes(&mut parser, &mut processor, data);
    }

    #[test]
    fn test_vt_sgr_bold() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[1mA");
        assert!(grid.row(0).unwrap()[0].style.bold);
    }

    #[test]
    fn test_vt_sgr_reverse() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[7mA");
        assert!(grid.row(0).unwrap()[0].style.reverse);
    }

    #[test]
    fn test_vt_sgr_underline() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[4mA");
        assert!(grid.row(0).unwrap()[0].style.underline);
    }

    #[test]
    fn test_vt_sgr_reset() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[1m");
        feed(&mut grid, b"\x1b[0m");
        feed(&mut grid, b"A");
        assert!(!grid.row(0).unwrap()[0].style.bold);
    }

    #[test]
    fn test_vt_sgr_multiple_params() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[1;4mA");
        let cell = &grid.row(0).unwrap()[0];
        assert!(cell.style.bold);
        assert!(cell.style.underline);
    }

    #[test]
    fn test_vt_sgr_strikethrough() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[9mA");
        assert!(grid.row(0).unwrap()[0].style.strikethrough);
    }

    #[test]
    fn test_sgr_fg_red() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[31mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 128, g: 0, b: 0 }));
    }

    #[test]
    fn test_sgr_bg_green() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[42mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 128, b: 0 }));
    }

    #[test]
    fn test_sgr_fg_bg_reset_to_default() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[31;42m");
        feed(&mut grid, b"\x1b[39;49mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
    }

    #[test]
    fn test_sgr_bright_fg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[91mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 255, g: 0, b: 0 }));
    }

    #[test]
    fn test_sgr_bright_bg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[100mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(
            style.bg,
            Some(Color {
                r: 128,
                g: 128,
                b: 128
            })
        );
    }

    #[test]
    fn test_sgr_256_fg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[38;5;1mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 128, g: 0, b: 0 }));
    }

    #[test]
    fn test_sgr_256_bg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[48;5;2mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 128, b: 0 }));
    }

    #[test]
    fn test_sgr_256_cube_first() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[38;5;16mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 0, g: 0, b: 0 }));
    }

    #[test]
    fn test_sgr_256_grayscale() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[38;5;232mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 8, g: 8, b: 8 }));
    }

    #[test]
    fn test_sgr_rgb_fg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[38;2;255;128;0mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(
            style.fg,
            Some(Color {
                r: 255,
                g: 128,
                b: 0
            })
        );
    }

    #[test]
    fn test_sgr_rgb_bg() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[48;2;0;0;255mA");
        let style = grid.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 0, b: 255 }));
    }

    #[test]
    fn test_sgr_italic_off() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[3m");
        feed(&mut grid, b"\x1b[23mA");
        assert!(!grid.row(0).unwrap()[0].style.italic);
    }

    #[test]
    fn test_sgr_underline_off() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[4m\x1b[24mA");
        assert!(!grid.row(0).unwrap()[0].style.underline);
    }

    #[test]
    fn test_sgr_reverse_off() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[7m\x1b[27mA");
        assert!(!grid.row(0).unwrap()[0].style.reverse);
    }
}
