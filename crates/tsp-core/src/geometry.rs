//! Pad geometry, computed from the profile (never measured in the DOM).
//!
//! The grid is in half-key units: `gridColumns: 6` is three standard keys
//! (span 2). Buttons flow left to right, wrapping when a row is full. Height
//! is `min(content, work area − 2 × margin)`; when content is taller, rows
//! shrink evenly to a 56 px floor and a warning is raised. The pad never
//! scrolls.

pub const PADDING: u32 = 10;
pub const HEADER: u32 = 40;
pub const MARGIN: u32 = 12;
pub const MIN_ROW: u32 = 56;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub row_height: u32,
    pub rows: u32,
    /// Button rects relative to the pad's top-left, in profile order.
    pub buttons: Vec<Rect>,
    /// Set when rows were shrunk or still do not fit at the floor.
    pub warning: Option<String>,
}

pub struct GridSpec<'a> {
    pub grid_columns: u32,
    pub button_width: u32,
    pub button_height: u32,
    pub gap: u32,
    pub spans: &'a [u32],
}

pub fn layout(spec: &GridSpec, work_area_height: u32) -> Layout {
    let gap = spec.gap;
    let col = spec.button_width.saturating_sub(gap) / 2;
    let span_w = |span: u32| span * col + span.saturating_sub(1) * gap;
    let inner_w = span_w(spec.grid_columns);

    // Flow buttons into rows of half-key columns.
    let mut cells: Vec<(u32, u32, u32)> = Vec::with_capacity(spec.spans.len()); // (row, start_col, span)
    let (mut row, mut used) = (0u32, 0u32);
    for &span in spec.spans {
        let span = span.clamp(1, spec.grid_columns);
        if used + span > spec.grid_columns {
            row += 1;
            used = 0;
        }
        cells.push((row, used, span));
        used += span;
    }
    let rows = if cells.is_empty() { 0 } else { row + 1 };

    let fixed = 2 * PADDING + HEADER + gap + rows.saturating_sub(1) * gap;
    let available = work_area_height.saturating_sub(2 * MARGIN);
    let natural = fixed + rows * spec.button_height;
    let (row_height, warning) = if natural <= available || rows == 0 {
        (spec.button_height, None)
    } else {
        let fit = available.saturating_sub(fixed) / rows;
        if fit >= MIN_ROW {
            (fit, Some(format!("rows shrunk from {} to {fit} px to fit the screen", spec.button_height)))
        } else {
            (MIN_ROW, Some(format!("pad does not fit the screen even at the {MIN_ROW} px row floor")))
        }
    };

    let top = PADDING + HEADER + gap;
    let buttons = cells
        .iter()
        .map(|&(r, c, span)| Rect {
            x: PADDING + c * (col + gap),
            y: top + r * (row_height + gap),
            w: span_w(span),
            h: row_height,
        })
        .collect();
    Layout {
        width: inner_w + 2 * PADDING,
        height: fixed + rows * row_height,
        row_height,
        rows,
        buttons,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The R1 starter: 3 rows of digits, 0 + ., Enter, Up/Down, Ctrl±, ×10, Tab, Undo/Redo.
    const R1_SPANS: &[u32] = &[2, 2, 2, 2, 2, 2, 2, 2, 2, 4, 2, 6, 3, 3, 3, 3, 3, 3, 6, 3, 3];

    fn spec(spans: &[u32]) -> GridSpec<'_> {
        GridSpec { grid_columns: 6, button_width: 92, button_height: 78, gap: 8, spans }
    }

    #[test]
    fn width_matches_the_approved_mockup() {
        // 3 × 92 + 2 × 8 = 292 inner + 2 × 10 padding = 312.
        let l = layout(&spec(R1_SPANS), 2000);
        assert_eq!(l.width, 312);
    }

    #[test]
    fn r1_starter_has_ten_rows_and_fits_a_tall_screen_unshrunk() {
        let l = layout(&spec(R1_SPANS), 2000);
        assert_eq!(l.rows, 10);
        assert_eq!(l.row_height, 78);
        // padding + header + gap + 10 rows + 9 gaps + padding
        assert_eq!(l.height, 10 + 40 + 8 + 10 * 78 + 9 * 8 + 10);
        assert_eq!(l.warning, None);
    }

    #[test]
    fn buttons_flow_and_wrap_in_half_key_units() {
        let l = layout(&spec(R1_SPANS), 2000);
        let col = (92 - 8) / 2; // 42 px half-key column
        let top = 10 + 40 + 8;
        assert_eq!(l.buttons[0], Rect { x: 10, y: top, w: 92, h: 78 });
        assert_eq!(l.buttons[1], Rect { x: 10 + 92 + 8, y: top, w: 92, h: 78 });
        // "0" spans 4 half-columns: 4 × 42 + 3 × 8 = 192
        assert_eq!(l.buttons[9], Rect { x: 10, y: top + 3 * 86, w: 4 * col + 3 * 8, h: 78 });
        // Enter spans the full row
        assert_eq!(l.buttons[11].w, 292);
        // Up (span 3) and Down share a row, each 3 × 42 + 2 × 8 = 142
        assert_eq!((l.buttons[12].w, l.buttons[13].x), (142, 10 + 142 + 8));
    }

    #[test]
    fn surface_work_area_shrinks_rows_evenly_with_a_warning() {
        // Surface Pro 8-11: 960 CSS px − 48 px taskbar = 912 work area; 888 usable after margins.
        let l = layout(&spec(R1_SPANS), 912);
        assert!(l.height <= 912 - 2 * MARGIN, "height {}", l.height);
        assert!(l.row_height < 78 && l.row_height >= MIN_ROW, "row {}", l.row_height);
        assert!(l.warning.is_some());
        assert!(l.buttons.iter().all(|b| b.h == l.row_height));
    }

    #[test]
    fn rows_never_go_below_the_floor() {
        let l = layout(&spec(R1_SPANS), 500);
        assert_eq!(l.row_height, MIN_ROW);
        assert!(l.warning.as_deref().unwrap().contains("does not fit"));
    }

    #[test]
    fn a_span_wider_than_what_is_left_starts_a_new_row() {
        let l = layout(&spec(&[2, 2, 4]), 2000);
        assert_eq!(l.rows, 2);
        assert_eq!(l.buttons[2].x, 10);
    }
}
