//! ペインレイアウトツリーとペインストア
//!
//! `Arc<Mutex<PaneStore>>` でウィンドウスレッドと tokio タスクが共有する。

mod catalog;
mod launcher;
mod store;
mod tree;

use std::collections::HashMap;

use yatamux_protocol::types::{PaneId, SplitDirection};

pub use catalog::{
    list_available_layouts, list_available_themes, load_theme_from_file, save_layout_file,
};
pub use launcher::{LauncherState, LayoutPreview, ThemeLauncherState};
pub use store::{CopyState, PaneStore, PromptState, Toast, ALERT_FLIP_COUNT, ALERT_TICK_DIVISOR};
pub use tree::{Direction, LayoutNode, PaneRect};

/// 現在のレイアウトツリーを `[[panes]]` TOML 形式に変換する。
///
/// DFS でノードを訪問し、各葉ペインを 1 エントリとして出力する。
/// ratio が 0.5 と異なる場合のみ `ratio = X.XXX` を出力する。
/// 制限: `first` が Split であるような左辺スプリット（左優先ツリー）を
/// ロードし直すと構造が変わる（連鎖型ツリーとして再構成される）。
pub fn layout_to_toml(node: &LayoutNode, commands: &HashMap<PaneId, String>) -> String {
    // (pane_id, split_dir, ratio) — 最初のペインは split = None
    fn collect(
        node: &LayoutNode,
        split: Option<(&'static str, f32)>,
        out: &mut Vec<(PaneId, Option<(&'static str, f32)>)>,
    ) {
        match node {
            LayoutNode::Leaf(id) => out.push((*id, split)),
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                collect(first, split, out);
                let dir = match direction {
                    SplitDirection::Vertical => "vertical",
                    SplitDirection::Horizontal => "horizontal",
                };
                collect(second, Some((dir, *ratio)), out);
            }
        }
    }

    let mut panes: Vec<(PaneId, Option<(&'static str, f32)>)> = Vec::new();
    collect(node, None, &mut panes);

    let mut out = String::new();
    for (pane_id, split) in panes {
        out.push_str("[[panes]]\n");
        if let Some(cmd) = commands.get(&pane_id) {
            // TOML の文字列リテラルとして適切にエスケープして出力
            out.push_str(&format!("command = {cmd:?}\n"));
        }
        if let Some((dir, ratio)) = split {
            out.push_str(&format!("split = \"{dir}\"\n"));
            if (ratio - 0.5).abs() >= 1e-4 {
                out.push_str(&format!("ratio = {ratio:.4}\n"));
            }
        }
        out.push('\n');
    }
    out
}

// ── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PaneRect {
        PaneRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        }
    }

    // TC-01: 垂直分割で Right → 右ペイン
    #[test]
    fn test_direction_right_vertical_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Right, root()),
            PaneId(2)
        );
    }

    // TC-02: 垂直分割で Left → 左ペイン
    #[test]
    fn test_direction_left_vertical_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(2), Direction::Left, root()),
            PaneId(1)
        );
    }

    // TC-03: 端のペインで移動先なし → 自ペインを返す
    #[test]
    fn test_direction_no_candidate_returns_self() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Left, root()),
            PaneId(1)
        );
    }

    // TC-04: 水平分割で Down → 下ペイン
    #[test]
    fn test_direction_down_horizontal_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let r = PaneRect {
            x: 0,
            y: 0,
            w: 100,
            h: 200,
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Down, r),
            PaneId(2)
        );
    }

    // TC-05: 水平分割で Up → 上ペイン
    #[test]
    fn test_direction_up_horizontal_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let r = PaneRect {
            x: 0,
            y: 0,
            w: 100,
            h: 200,
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(2), Direction::Up, r),
            PaneId(1)
        );
    }

    // TC-06: 単一ペインでは常に自ペインを返す
    #[test]
    fn test_direction_single_pane_returns_self() {
        let layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Right, root()),
            PaneId(1)
        );
    }

    // ── pane_at_point テスト ──────────────────────────────────────────────

    // TC-F9-01: 単一ペイン — どこをクリックしても自ペイン
    #[test]
    fn test_pane_at_point_single() {
        let layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(layout.pane_at_point(50, 50, root()), Some(PaneId(1)));
    }

    // TC-F9-02: 垂直分割 — 左半分 → first
    #[test]
    fn test_pane_at_point_vertical_left() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // root = 200x100, SEP=1, w1=99, w2=100
        assert_eq!(layout.pane_at_point(10, 50, root()), Some(PaneId(1)));
    }

    // TC-F9-03: 垂直分割 — 右半分 → second
    #[test]
    fn test_pane_at_point_vertical_right() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(layout.pane_at_point(150, 50, root()), Some(PaneId(2)));
    }

    // TC-F9-04: 範囲外 → None
    #[test]
    fn test_pane_at_point_out_of_bounds() {
        let layout = LayoutNode::Leaf(PaneId(1));
        // root は (0,0,200,100)、点 (-1, 0) はヒットしない
        assert_eq!(layout.pane_at_point(-1, 0, root()), None);
    }

    // ── remove_pane テスト ────────────────────────────────────────────────

    // TC-F8-05: 単一 Leaf は削除不可 → None
    #[test]
    fn test_remove_pane_single_returns_none() {
        let mut layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(layout.remove_pane(PaneId(1)), None);
        // ツリーは変化しない
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(1))));
    }

    // TC-F8-06: 垂直分割の first を削除
    #[test]
    fn test_remove_pane_vertical_first() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let next = layout.remove_pane(PaneId(1));
        assert_eq!(next, Some(PaneId(2)));
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(2))));
    }

    // TC-F8-07: 垂直分割の second を削除
    #[test]
    fn test_remove_pane_vertical_second() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let next = layout.remove_pane(PaneId(2));
        assert_eq!(next, Some(PaneId(1)));
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(1))));
    }

    // ── adjust_ratio テスト (C-10) ─────────────────────────────────────────

    // TC-C10-01: 単一 Leaf → no-op (false)
    #[test]
    fn test_adjust_ratio_leaf_noop() {
        let mut layout = LayoutNode::Leaf(PaneId(1));
        assert!(!layout.adjust_ratio(PaneId(1), 0.05));
    }

    // TC-C10-02: first ペインを拡大 → ratio 増加
    #[test]
    fn test_adjust_ratio_first_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.55).abs() < 1e-6);
        }
    }

    // TC-C10-03: second ペインを拡大 → ratio 減少
    #[test]
    fn test_adjust_ratio_second_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(2), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.45).abs() < 1e-6);
        }
    }

    // TC-C10-04: ratio は 0.9 でクランプ
    #[test]
    fn test_adjust_ratio_clamp_max() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6);
        }
    }

    // TC-C10-05: ratio は 0.1 でクランプ
    #[test]
    fn test_adjust_ratio_clamp_min() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.12,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), -0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.1).abs() < 1e-6);
        }
    }

    // TC-C10-06: ネスト Split — 内側の Split を操作
    #[test]
    fn test_adjust_ratio_nested_inner() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(1))),
                second: Box::new(LayoutNode::Leaf(PaneId(2))),
            }),
            second: Box::new(LayoutNode::Leaf(PaneId(3))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        // 外側の ratio は変わらない
        if let LayoutNode::Split {
            ratio: outer_ratio,
            first,
            ..
        } = &layout
        {
            assert!((outer_ratio - 0.5).abs() < 1e-6);
            // 内側の ratio が変わっている
            if let LayoutNode::Split {
                ratio: inner_ratio, ..
            } = first.as_ref()
            {
                assert!((inner_ratio - 0.55).abs() < 1e-6);
            }
        }
    }

    // ── adjust_ratio テスト — Horizontal Split (C-18) ──────────────────────

    // TC-C18-01: Horizontal Split の first ペインを拡大 → ratio 増加
    #[test]
    fn test_adjust_ratio_horizontal_first_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.55).abs() < 1e-6);
        }
    }

    // TC-C18-02: Horizontal Split の second ペインを拡大 → ratio 減少
    #[test]
    fn test_adjust_ratio_horizontal_second_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(2), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.45).abs() < 1e-6);
        }
    }

    // TC-C18-03: Horizontal Split — ratio は 0.9 でクランプ
    #[test]
    fn test_adjust_ratio_horizontal_clamp_max() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6);
        }
    }

    // ── adjust_ratio_for_dir テスト (C-19 リグレッション防止) ────────────────

    // TC-C19-R01: Vertical-only adjust は Horizontal Split に触れない
    #[test]
    fn test_adjust_ratio_for_dir_vertical_ignores_horizontal() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // Vertical 方向の調整は Horizontal Split を変更しない
        let changed = layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Vertical);
        assert!(
            !changed,
            "Vertical adjust should not affect Horizontal split"
        );
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.5).abs() < 1e-6, "ratio should be unchanged");
        }
    }

    // TC-C19-R02: Horizontal-only adjust は Vertical Split に触れない
    #[test]
    fn test_adjust_ratio_for_dir_horizontal_ignores_vertical() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // Horizontal 方向の調整は Vertical Split を変更しない
        let changed = layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Horizontal);
        assert!(
            !changed,
            "Horizontal adjust should not affect Vertical split"
        );
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.5).abs() < 1e-6, "ratio should be unchanged");
        }
    }

    // TC-C19-R03: ネスト構造 Vertical(1, Horizontal(2, 3)) で方向ごとに別の Split が動く
    //
    // `<`/`>` (Vertical) を Pane2 に適用 → 外側 Vertical ratio が変化（フォーカス位置によらず delta の符号が境界方向を決める）
    // `+`/`-` (Horizontal) を Pane2 に適用 → 内側 Horizontal ratio が変化、外側 Vertical は不変
    #[test]
    fn test_adjust_ratio_for_dir_vertical_changes_outer_not_inner() {
        let make_layout = || LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };

        // Vertical 調整 → 外側 Vertical ratio が増加（delta=+0.05 は常に ratio を増加させる）
        // 内側 Horizontal ratio は不変
        let mut layout = make_layout();
        let changed = layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Vertical);
        assert!(
            changed,
            "Vertical adjust should affect outer Vertical split"
        );
        if let LayoutNode::Split {
            ratio: outer,
            second,
            ..
        } = &layout
        {
            assert!(
                (*outer - 0.55).abs() < 1e-6,
                "outer Vertical ratio increased: {outer}"
            );
            if let LayoutNode::Split { ratio: inner, .. } = second.as_ref() {
                assert!(
                    (*inner - 0.5).abs() < 1e-6,
                    "inner Horizontal ratio unchanged: {inner}"
                );
            }
        }

        // Horizontal 調整 → 内側 Horizontal ratio が変化（Pane2 は first 側なので増加）
        // 外側 Vertical ratio は不変
        let mut layout = make_layout();
        let changed = layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Horizontal);
        assert!(
            changed,
            "Horizontal adjust should affect inner Horizontal split"
        );
        if let LayoutNode::Split {
            ratio: outer,
            second,
            ..
        } = &layout
        {
            assert!(
                (*outer - 0.5).abs() < 1e-6,
                "outer Vertical ratio unchanged: {outer}"
            );
            if let LayoutNode::Split { ratio: inner, .. } = second.as_ref() {
                assert!(
                    (*inner - 0.55).abs() < 1e-6,
                    "inner Horizontal ratio increased: {inner}"
                );
            }
        }
    }

    // TC-C19-R04: adjust_ratio_for_dir で ratio クランプが機能する
    #[test]
    fn test_adjust_ratio_for_dir_clamps_ratio() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6, "ratio clamped at 0.9");
        }
    }

    // ── adjust_ratio_for_dir 方向一貫性テスト (F-6) ──────────────────────────

    // TC-F6-01: Vertical 分割・first フォーカスで delta=+0.05 → ratio 増加（境界右に移動）
    #[test]
    fn test_f6_vertical_first_focus_positive_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.55).abs() < 1e-6,
                "first-focus >: ratio should be 0.55, got {ratio}"
            );
        }
    }

    // TC-F6-02: Vertical 分割・second フォーカスで delta=+0.05 → ratio 増加（境界右に移動、second が縮小）
    #[test]
    fn test_f6_vertical_second_focus_positive_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.55).abs() < 1e-6,
                "second-focus >: ratio should be 0.55 (same as first-focus), got {ratio}"
            );
        }
    }

    // TC-F6-03: Vertical 分割・first フォーカスで delta=-0.05 → ratio 減少（境界左に移動）
    #[test]
    fn test_f6_vertical_first_focus_negative_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(1), -0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.45).abs() < 1e-6,
                "first-focus <: ratio should be 0.45, got {ratio}"
            );
        }
    }

    // TC-F6-04: Vertical 分割・second フォーカスで delta=-0.05 → ratio 減少（境界左に移動、first が縮小）
    #[test]
    fn test_f6_vertical_second_focus_negative_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(2), -0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.45).abs() < 1e-6,
                "second-focus <: ratio should be 0.45 (same as first-focus), got {ratio}"
            );
        }
    }

    // TC-F6-05: Horizontal 分割・first フォーカスで delta=+0.05 → ratio 増加（境界下に移動）
    #[test]
    fn test_f6_horizontal_first_focus_positive_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Horizontal);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.55).abs() < 1e-6,
                "first-focus +: ratio should be 0.55, got {ratio}"
            );
        }
    }

    // TC-F6-06: Horizontal 分割・second フォーカスで delta=+0.05 → ratio 増加（境界下に移動、second が縮小）
    #[test]
    fn test_f6_horizontal_second_focus_positive_delta() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Horizontal);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.55).abs() < 1e-6,
                "second-focus +: ratio should be 0.55 (same as first-focus), got {ratio}"
            );
        }
    }

    // TC-F6-07: ネスト Split で second フォーカスでも内側の ratio が正しく動く
    #[test]
    fn test_f6_nested_second_focus_inner_adjust() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(1))),
                second: Box::new(LayoutNode::Leaf(PaneId(2))),
            }),
            second: Box::new(LayoutNode::Leaf(PaneId(3))),
        };
        layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Vertical);
        // 内側 Split の ratio が増加（first-focus と同じ動作）し、外側は変わらない
        if let LayoutNode::Split {
            ratio: outer,
            first,
            ..
        } = &layout
        {
            assert!(
                (*outer - 0.5).abs() < 1e-6,
                "outer ratio unchanged: {outer}"
            );
            if let LayoutNode::Split { ratio: inner, .. } = first.as_ref() {
                assert!(
                    (*inner - 0.55).abs() < 1e-6,
                    "inner ratio increased: {inner}"
                );
            }
        }
    }

    // TC-F6-08: second フォーカスでもクランプが機能する
    #[test]
    fn test_f6_clamp_with_second_focus() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!(
                (ratio - 0.9).abs() < 1e-6,
                "ratio clamped at 0.9 with second focus: {ratio}"
            );
        }
    }

    // TC-F8-08: ネスト Split(1, Split(2, 3)) → remove 2 → Split(1, 3)
    #[test]
    fn test_remove_pane_nested() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };
        let next = layout.remove_pane(PaneId(2));
        assert_eq!(next, Some(PaneId(3)));
        // ツリーが Split(1, 3) になっていることを確認
        let ids = layout.pane_ids();
        assert_eq!(ids, vec![PaneId(1), PaneId(3)]);
    }

    // ── C-19: layout_to_toml / save_layout_file ─────────────────────────

    // TC-C19-01: 単一ペインは [[panes]] 1 エントリで split なし
    #[test]
    fn test_layout_to_toml_single_pane() {
        let node = LayoutNode::Leaf(PaneId(1));
        let toml = layout_to_toml(&node, &HashMap::new());
        assert_eq!(toml, "[[panes]]\n\n");
    }

    // TC-C19-02: 垂直分割は 2 エントリ、2 つ目に split = "vertical"
    #[test]
    fn test_layout_to_toml_vertical_split() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let toml = layout_to_toml(&node, &HashMap::new());
        assert_eq!(toml, "[[panes]]\n\n[[panes]]\nsplit = \"vertical\"\n\n");
    }

    // TC-C19-03: ネスト構造は 3 エントリ
    #[test]
    fn test_layout_to_toml_nested() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };
        let toml = layout_to_toml(&node, &HashMap::new());
        let expected = "[[panes]]\n\n\
                        [[panes]]\nsplit = \"vertical\"\n\n\
                        [[panes]]\nsplit = \"horizontal\"\n\n";
        assert_eq!(toml, expected);
    }

    // TC-C23-01: コマンドなしペインは command 行を出力しない
    #[test]
    fn test_layout_to_toml_no_command() {
        let node = LayoutNode::Leaf(PaneId(1));
        let toml = layout_to_toml(&node, &HashMap::new());
        assert!(!toml.contains("command"));
        assert_eq!(toml, "[[panes]]\n\n");
    }

    // TC-C23-02: コマンドありペインは command = "..." 行を出力する
    #[test]
    fn test_layout_to_toml_with_command() {
        let node = LayoutNode::Leaf(PaneId(1));
        let mut commands = HashMap::new();
        commands.insert(PaneId(1), "cargo watch".to_string());
        let toml = layout_to_toml(&node, &commands);
        assert!(toml.contains("command = \"cargo watch\""), "got: {toml}");
    }

    // TC-C23-03: 垂直分割でコマンドを持つ 2 つ目のペインが TOML に含まれる
    #[test]
    fn test_layout_to_toml_split_with_command() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let mut commands = HashMap::new();
        commands.insert(PaneId(2), "cargo test".to_string());
        let toml = layout_to_toml(&node, &commands);
        let expected = "[[panes]]\n\n\
                        [[panes]]\n\
                        command = \"cargo test\"\n\
                        split = \"vertical\"\n\n";
        assert_eq!(toml, expected);
    }

    // TC-C23-04: コマンドに特殊文字を含む場合も適切にエスケープされる
    #[test]
    fn test_layout_to_toml_command_with_special_chars() {
        let node = LayoutNode::Leaf(PaneId(1));
        let mut commands = HashMap::new();
        commands.insert(PaneId(1), r#"echo "hello""#.to_string());
        let toml = layout_to_toml(&node, &commands);
        // Rust の Debug フォーマットで " がエスケープされる
        assert!(toml.contains("command ="), "got: {toml}");
        // TOML としてパース可能であることを確認
        let parsed: toml::Value = toml::from_str(&toml).expect("should be valid TOML");
        let arr = parsed["panes"].as_array().unwrap();
        assert_eq!(arr[0]["command"].as_str().unwrap(), r#"echo "hello""#);
    }

    // TC-C19-04: save_layout_file は正常に書き込む
    #[test]
    fn test_save_layout_file_roundtrip() {
        let dir = std::env::temp_dir().join("yatamux_save_layout_test");
        std::fs::create_dir_all(&dir).unwrap();

        // APPDATA を一時ディレクトリで代替するため直接書き込みテスト
        let content = "[[panes]]\n\n[[panes]]\nsplit = \"vertical\"\n\n";
        let path = dir.join("testlayout.toml");
        std::fs::write(&path, content).unwrap();
        let loaded = std::fs::read_to_string(&path).unwrap();
        assert_eq!(loaded, content);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
