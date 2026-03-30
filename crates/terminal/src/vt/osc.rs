use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};

pub(super) fn dispatch_osc(
    title: &mut Option<String>,
    notification: &mut Option<String>,
    clipboard_data: &mut Option<Vec<u8>>,
    command_finished: &mut Option<Option<i32>>,
    params: &[&[u8]],
) {
    if params.is_empty() {
        return;
    }

    let cmd = std::str::from_utf8(params[0]).unwrap_or("");
    match cmd {
        "0" | "2" => {
            if let Some(title_bytes) = params.get(1) {
                if let Ok(value) = std::str::from_utf8(title_bytes) {
                    *title = Some(value.to_string());
                }
            }
        }
        "9" => {
            if let Some(body) = params.get(1) {
                if let Ok(value) = std::str::from_utf8(body) {
                    *notification = Some(value.to_string());
                }
            }
        }
        "99" | "777" => {
            if let Some(body) = params.get(1) {
                if let Ok(value) = std::str::from_utf8(body) {
                    *notification = Some(value.to_string());
                }
            }
        }
        "133" => {
            let subcode = params
                .get(1)
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or("");
            if subcode == "D" {
                let exit_code = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .and_then(|s| s.parse::<i32>().ok());
                *command_finished = Some(exit_code);
            } else if let Some(code_str) = subcode.strip_prefix("D;") {
                let exit_code = code_str.parse::<i32>().ok();
                *command_finished = Some(exit_code);
            }
        }
        "52" => {
            let kind = params
                .get(1)
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or("");
            if kind == "c" {
                let b64 = params
                    .get(2)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if let Ok(decoded) = BASE64_STANDARD.decode(b64) {
                    *clipboard_data = Some(decoded);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use crate::vt::{feed_bytes, VtProcessor};
    use crate::width::CjkWidthConfig;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use vte::Parser;

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    fn osc52_proc(data: &[u8]) -> Option<Vec<u8>> {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, data);
        processor.clipboard_data
    }

    #[test]
    fn test_vt_osc_title() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]2;My Terminal Title\x07");
        assert_eq!(processor.title.as_deref(), Some("My Terminal Title"));
    }

    #[test]
    fn test_vt_osc_notification_9() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]9;Build complete\x07");
        assert_eq!(processor.notification.as_deref(), Some("Build complete"));
    }

    #[test]
    fn test_vt_osc_133_command_finished_without_exit_code() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]133;D\x07");
        assert_eq!(processor.command_finished, Some(None));
    }

    #[test]
    fn test_vt_osc_133_command_finished_with_exit_code() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]133;D;7\x07");
        assert_eq!(processor.command_finished, Some(Some(7)));
    }

    #[test]
    fn test_vt_osc_notification_777() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]777;notify;Test\x07");
        assert_eq!(processor.notification.as_deref(), Some("notify"));
    }

    #[test]
    fn test_osc52_ascii_bel() {
        let result = osc52_proc(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    #[test]
    fn test_osc52_utf8_japanese() {
        let text = "こんにちは".as_bytes();
        let b64 = BASE64_STANDARD.encode(text);
        let sequence = format!("\x1b]52;c;{}\x07", b64);
        let result = osc52_proc(sequence.as_bytes());
        assert_eq!(result, Some(text.to_vec()));
    }

    #[test]
    fn test_osc52_st_terminator() {
        let result = osc52_proc(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    #[test]
    fn test_osc52_empty_data() {
        let result = osc52_proc(b"\x1b]52;c;\x07");
        assert_eq!(result, Some(b"".to_vec()));
    }

    #[test]
    fn test_osc52_non_clipboard_type_ignored() {
        let result = osc52_proc(b"\x1b]52;p;aGVsbG8=\x07");
        assert_eq!(result, None);
    }

    #[test]
    fn test_osc52_invalid_base64() {
        let result = osc52_proc(b"\x1b]52;c;!!!invalid!!!\x07");
        assert_eq!(result, None);
    }

    #[test]
    fn test_osc52_overwrite() {
        let mut grid = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(&mut grid);
        feed_bytes(&mut parser, &mut processor, b"\x1b]52;c;Zmlyc3Q=\x07");
        feed_bytes(&mut parser, &mut processor, b"\x1b]52;c;c2Vjb25k\x07");
        assert_eq!(processor.clipboard_data, Some(b"second".to_vec()));
    }
}
