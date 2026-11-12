//! Pad geometry, computed from the profile (never measured in the DOM).
//!
//! [`fit`] is the schema 2 rule: a grid of cells, scaled, with v1's height
//! rule kept exactly (rows shrink evenly to fit the work area, never below a
//! 56 px floor, which can enlarge small cells) and a width clamp that is new
//! in v2. [`layout`] is v1's flow layout, kept as the reference the golden
//! tests compare converted v1 profiles against.

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

/// Extra space around the grid in edit mode, so the resize handle stays reachable.
pub const EDIT_MARGIN: f64 = 44.0;
/// Edit-mode header height floor (Done must be a 44 px target at any scale).
pub const EDIT_HEADER_MIN: f64 = 44.0;

/// Grid inputs from a schema 2 pad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub gap: u32,
    pub scale: f64,
}

/// A button's rectangle in grid cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cells { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

#[derive(Debug, Clone, PartialEq)]
pub struct Fit {
    /// Window size in logical px (clamped to the work area).
    pub width: u32,
    pub height: u32,
    /// Header height (row 1 + row 2) in logical px.
    pub header: f64,
    /// Rendered row height, after the fit rule.
    pub row_height: f64,
    /// Top-left of cell (0, 0), cell width and gap, in logical px (for edit-mode hit tests).
    pub grid_left: f64,
    pub grid_top: f64,
    pub cell_width: f64,
    pub gap: f64,
    /// Button rects relative to the window's top-left, in input order.
    pub buttons: Vec<Rect>,
    /// Rows shrunk, or the pad clipped to the screen.
    pub warning: Option<String>,
    /// The natural pad is larger than the work area; right columns or bottom rows are cut off.
    pub clipped: bool,
}

/// Schema 2 fit rule. All maths in f64 logical px; each rect edge is rounded once.
pub fn fit(grid: &Grid, cells: &[Cells], work_area: (u32, u32), edit: bool) -> Fit {
    let s = grid.scale;
    let pad = f64::from(PADDING) * s;
    let gap = f64::from(grid.gap) * s;
    let cw = f64::from(grid.cell_width) * s;
    let ch = f64::from(grid.cell_height) * s;
    let header = if edit { (f64::from(HEADER) * s).max(EDIT_HEADER_MIN) } else { f64::from(HEADER) * s };
    let side = if edit { pad + EDIT_MARGIN } else { pad };
    let bottom = side;
    let rows = f64::from(grid.rows);
    let columns = f64::from(grid.columns);

    let top = pad + header + gap;
    let fixed = top + (rows - 1.0).max(0.0) * gap + bottom;
    let available = f64::from(work_area.1.saturating_sub(2 * MARGIN));
    let natural = fixed + rows * ch;
    let (row_height, mut warning) = if natural <= available || grid.rows == 0 {
        (ch, None)
    } else {
        let fitted = ((available - fixed) / rows).floor();
        if fitted >= f64::from(MIN_ROW) {
            (fitted, Some(format!("rows shrunk from {} to {fitted} px to fit the screen", ch.round())))
        } else {
            (f64::from(MIN_ROW), None)
        }
    };

    let width = side * 2.0 + columns * cw + (columns - 1.0).max(0.0) * gap;
    let height = fixed + rows * row_height;
    let max_w = f64::from(work_area.0.saturating_sub(2 * MARGIN));
    let clipped = width > max_w || height > available;
    if clipped {
        warning = Some(format!(
            "Pad is larger than the screen ({}×{} px, room for {}×{}): lower scale, rows or columns",
            width.round(),
            height.round(),
            max_w,
            available
        ));
    }
    let buttons = cells
        .iter()
        .map(|c| {
            let left = side + f64::from(c.x) * (cw + gap);
            let right = left + f64::from(c.w) * cw + f64::from(c.w.saturating_sub(1)) * gap;
            let upper = top + f64::from(c.y) * (row_height + gap);
            let lower = upper + f64::from(c.h) * row_height + f64::from(c.h.saturating_sub(1)) * gap;
            let (x, y) = (left.round() as u32, upper.round() as u32);
            Rect { x, y, w: right.round() as u32 - x, h: lower.round() as u32 - y }
        })
        .collect();
    Fit {
        width: width.min(max_w).round() as u32,
        height: height.min(available).round() as u32,
        header,
        row_height,
        grid_left: side,
        grid_top: top,
        cell_width: cw,
        gap,
        buttons,
        warning,
        clipped,
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

    // ---------- schema 2 fit rule ----------

    fn r1_grid() -> (Grid, Vec<Cells>) {
        // The R1 starter as v1 → v2 conversion places it (6 half-columns of 42 px).
        let spans = R1_SPANS;
        let (mut row, mut used) = (0, 0);
        let mut cells = Vec::new();
        for &span in spans {
            if used + span > 6 {
                row += 1;
                used = 0;
            }
            cells.push(Cells { x: used, y: row, w: span, h: 1 });
            used += span;
        }
        (Grid { columns: 6, rows: row + 1, cell_width: 42, cell_height: 78, gap: 8, scale: 1.0 }, cells)
    }

    #[test]
    fn fit_matches_v1_for_the_converted_r1_profile() {
        let (grid, cells) = r1_grid();
        for wa in [912, 2000] {
            let v1 = layout(&spec(R1_SPANS), wa);
            let v2 = fit(&grid, &cells, (1920, wa), false);
            assert_eq!((v2.width, v2.height), (v1.width, v1.height), "work area {wa}");
            assert_eq!(v2.buttons, v1.buttons, "work area {wa}");
            assert_eq!(v2.row_height, f64::from(v1.row_height));
            assert!(!v2.clipped);
        }
        assert_eq!(fit(&grid, &cells, (1920, 912), false).row_height, 74.0);
        assert_eq!(fit(&grid, &cells, (1920, 2000), false).row_height, 78.0);
    }

    #[test]
    fn overflowing_small_cells_render_at_the_56_px_floor_like_v1() {
        let spans = [2u32; 30];
        let v1 = layout(&GridSpec { grid_columns: 6, button_width: 92, button_height: 40, gap: 8, spans: &spans }, 500);
        let cells: Vec<Cells> = (0..30).map(|i| Cells { x: (i % 3) * 2, y: i / 3, w: 2, h: 1 }).collect();
        let v2 = fit(&Grid { columns: 6, rows: 10, cell_width: 42, cell_height: 40, gap: 8, scale: 1.0 }, &cells, (1920, 500), false);
        assert_eq!(v1.row_height, 56);
        assert_eq!(v2.row_height, 56.0);
        assert_eq!(v2.buttons, v1.buttons);
        assert!(v2.clipped, "still too tall at the floor");
        assert_eq!(v2.height, 500 - 2 * MARGIN);
    }

    #[test]
    fn a_pad_wider_than_the_screen_is_clamped_with_a_warning() {
        let g = Grid { columns: 12, rows: 2, cell_width: 200, cell_height: 60, gap: 8, scale: 1.0 };
        let f = fit(&g, &[Cells { x: 11, y: 0, w: 1, h: 1 }], (1280, 900), false);
        assert!(f.clipped);
        assert_eq!(f.width, 1280 - 2 * MARGIN);
        let w = f.warning.unwrap();
        assert!(w.contains("larger than the screen") && w.contains("1256"), "{w}");
        assert!(f.buttons[0].x > f.width, "right column is clipped, not squeezed");
    }

    #[test]
    fn scale_multiplies_cells_gap_padding_and_header_but_not_the_floor() {
        let g = |scale| Grid { columns: 2, rows: 1, cell_width: 100, cell_height: 80, gap: 10, scale };
        let cells = [Cells { x: 0, y: 0, w: 1, h: 1 }, Cells { x: 1, y: 0, w: 1, h: 1 }];
        let half = fit(&g(0.5), &cells, (1920, 2000), false);
        assert_eq!(half.buttons[0], Rect { x: 5, y: 5 + 20 + 5, w: 50, h: 40 });
        assert_eq!(half.buttons[1].x, 5 + 50 + 5);
        assert_eq!(half.width, 5 + 50 + 5 + 50 + 5);
        let double = fit(&g(2.0), &cells, (1920, 2000), false);
        assert_eq!(double.buttons[0], Rect { x: 20, y: 20 + 80 + 20, w: 200, h: 160 });
        // Floor is unscaled: at 0.5 an overflowing 40 px row still renders at 56.
        let tiny = fit(&Grid { columns: 1, rows: 40, cell_width: 100, cell_height: 80, gap: 0, scale: 0.5 }, &[], (1920, 900), false);
        assert_eq!(tiny.row_height, 56.0);
    }

    #[test]
    fn edit_mode_adds_the_margin_and_a_44_px_header_and_refits() {
        let (grid, cells) = r1_grid();
        let normal = fit(&grid, &cells, (1920, 912), false);
        let edit = fit(&grid, &cells, (1920, 912), true);
        assert_eq!(edit.width, normal.width + 2 * 44);
        assert_eq!(edit.header, 44.0);
        assert!(edit.row_height < normal.row_height && edit.row_height >= 56.0, "{}", edit.row_height);
        assert!(edit.height <= 912 - 2 * MARGIN);
        assert_eq!(edit.buttons[0].x, 10 + 44);
        // Scale 0.5 in edit mode: header stays 44, never overlapping keys.
        let small = fit(&Grid { scale: 0.5, ..grid }, &cells, (1920, 912), true);
        assert_eq!(small.header, 44.0);
        assert!(f64::from(small.buttons[0].y) >= 5.0 + 44.0);
    }

    #[test]
    fn fractional_scale_rounds_each_edge_once() {
        let g = Grid { columns: 3, rows: 1, cell_width: 42, cell_height: 78, gap: 8, scale: 1.1 };
        let cells = [Cells { x: 0, y: 0, w: 1, h: 1 }, Cells { x: 1, y: 0, w: 1, h: 1 }, Cells { x: 0, y: 0, w: 3, h: 1 }];
        let f = fit(&g, &cells, (1920, 2000), false);
        // Adjacent keys never overlap and the 3-wide key spans exactly from the first's left to the last's right.
        assert!(f.buttons[0].x + f.buttons[0].w <= f.buttons[1].x);
        assert_eq!(f.buttons[2].x, f.buttons[0].x);
    }
}
