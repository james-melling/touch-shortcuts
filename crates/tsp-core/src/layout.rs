//! Layout engine: move or resize one button, shoving the buttons it lands on
//! out of the way like home-screen icons. Pure and deterministic.
//!
//! Rules (docs/designs/in-app-editor.md, "Layout engine"):
//! 1. The target rect must lie inside the grid (`layout-out-of-grid`).
//! 2. Victims are the other buttons the target overlaps, by top-left (row, column).
//! 3. Free cells: everything except non-victim buttons and the target. The
//!    mover's vacated cells and the victims' own cells count as free.
//! 4. Each victim first tries to swap into the mover's vacated cells
//!    (top-left-most fit). Otherwise it takes the free fit nearest its old
//!    top-left (Manhattan), ties broken by [`tie_key`].
//! 5. If a victim has no fit, retry once with victims by descending area;
//!    then refuse (`layout-grid-full`) and move nothing.
//! 6. Non-victims never move; each victim is placed once.

use crate::geometry::Cells;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    OutOfGrid,
    /// No room for this button (by id).
    GridFull { id: String },
}

impl LayoutError {
    pub fn code(&self) -> &'static str {
        match self {
            LayoutError::OutOfGrid => "layout-out-of-grid",
            LayoutError::GridFull { .. } => "layout-grid-full",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub rect: Cells,
}

fn intersects(a: &Cells, b: &Cells) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

pub fn inside(r: &Cells, columns: u32, rows: u32) -> bool {
    r.w >= 1 && r.h >= 1 && r.x + r.w <= columns && r.y + r.h <= rows
}

/// Sort key among candidates at equal Manhattan distance (ascending): straight
/// below, below-right, below-left, right, left, straight above, above-right,
/// above-left, then lower row, then lower column.
pub fn tie_key(dx: i64, dy: i64, new_x: u32, new_y: u32) -> (bool, bool, bool, bool, u32, u32) {
    (dy <= 0, dy < 0, dx != 0, dx < 0, new_y, new_x)
}

struct Free {
    columns: u32,
    rows: u32,
    cells: Vec<bool>,
}

impl Free {
    fn idx(&self, x: u32, y: u32) -> usize {
        (y * self.columns + x) as usize
    }
    fn set(&mut self, r: &Cells, v: bool) {
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let i = self.idx(x, y);
                self.cells[i] = v;
            }
        }
    }
    fn fits(&self, r: &Cells) -> bool {
        inside(r, self.columns, self.rows) && (r.y..r.y + r.h).all(|y| (r.x..r.x + r.w).all(|x| self.cells[self.idx(x, y)]))
    }
}

/// Moves (or resizes) `items[mover]` to `target`. Returns every button's new
/// rect in input order, or an error with nothing moved.
pub fn place(columns: u32, rows: u32, items: &[Item], mover: usize, target: Cells) -> Result<Vec<Cells>, LayoutError> {
    if !inside(&target, columns, rows) {
        return Err(LayoutError::OutOfGrid);
    }
    let mut victims: Vec<usize> = (0..items.len()).filter(|&i| i != mover && intersects(&items[i].rect, &target)).collect();
    victims.sort_by_key(|&i| (items[i].rect.y, items[i].rect.x));
    match attempt(columns, rows, items, mover, target, &victims) {
        Ok(out) => Ok(out),
        Err(first) => {
            let mut by_area = victims.clone();
            by_area.sort_by_key(|&i| std::cmp::Reverse(items[i].rect.w * items[i].rect.h)); // stable: keeps (row, column) among equals
            if by_area == victims {
                return Err(first);
            }
            attempt(columns, rows, items, mover, target, &by_area).map_err(|_| first)
        }
    }
}

fn attempt(columns: u32, rows: u32, items: &[Item], mover: usize, target: Cells, order: &[usize]) -> Result<Vec<Cells>, LayoutError> {
    let mut free = Free { columns, rows, cells: vec![true; (columns * rows) as usize] };
    for (i, it) in items.iter().enumerate() {
        if i != mover && !order.contains(&i) {
            free.set(&it.rect, false);
        }
    }
    free.set(&target, false);
    let mut vacated = Free { columns, rows, cells: vec![false; (columns * rows) as usize] };
    vacated.set(&items[mover].rect, true);
    vacated.set(&target, false);

    let mut out: Vec<Cells> = items.iter().map(|it| it.rect).collect();
    out[mover] = target;
    for &v in order {
        let old = items[v].rect;
        let positions = || (0..=rows.saturating_sub(old.h)).flat_map(move |y| (0..=columns.saturating_sub(old.w)).map(move |x| Cells { x, y, w: old.w, h: old.h }));
        let swap = positions().find(|r| free.fits(r) && vacated.fits(r));
        let chosen = swap.or_else(|| {
            positions().filter(|r| free.fits(r)).min_by_key(|r| {
                let (dx, dy) = (i64::from(r.x) - i64::from(old.x), i64::from(r.y) - i64::from(old.y));
                (dx.abs() + dy.abs(), tie_key(dx, dy, r.x, r.y))
            })
        });
        let Some(r) = chosen else {
            return Err(LayoutError::GridFull { id: items[v].id.clone() });
        };
        free.set(&r, false);
        vacated.set(&r, false);
        out[v] = r;
    }
    Ok(out)
}

/// First free top-left in reading order where a `w`×`h` button fits, if any.
pub fn first_free(columns: u32, rows: u32, items: &[Cells], w: u32, h: u32) -> Option<Cells> {
    let mut free = Free { columns, rows, cells: vec![true; (columns * rows) as usize] };
    for r in items {
        free.set(r, false);
    }
    (0..rows).flat_map(|y| (0..columns).map(move |x| Cells { x, y, w, h })).find(|r| free.fits(r))
}

/// Nearest free fit to `from` by the same distance and tie rule as shoves.
pub fn nearest_free(columns: u32, rows: u32, items: &[Cells], from: Cells) -> Option<Cells> {
    let mut free = Free { columns, rows, cells: vec![true; (columns * rows) as usize] };
    for r in items {
        free.set(r, false);
    }
    (0..rows)
        .flat_map(|y| (0..columns).map(move |x| Cells { x, y, w: from.w, h: from.h }))
        .filter(|r| free.fits(r))
        .min_by_key(|r| {
            let (dx, dy) = (i64::from(r.x) - i64::from(from.x), i64::from(r.y) - i64::from(from.y));
            (dx.abs() + dy.abs(), tie_key(dx, dy, r.x, r.y))
        })
}

/// Device-pixel rect (left, top, right, bottom) for a window region.
pub type PxRect = (i32, i32, i32, i32);

/// Most rects a region accepts (keys + header + menu fit far below this).
pub const MAX_REGION_RECTS: usize = 200;

/// Converts logical rects `[x, y, w, h]` to device pixels for `SetWindowRgn`:
/// × scale, rounded outward (floor left/top, ceil right/bottom), clamped to
/// the window, capped at [`MAX_REGION_RECTS`], empty rects dropped.
pub fn region_rects(rects: &[[f64; 4]], scale: f64, window_px: (u32, u32)) -> Vec<PxRect> {
    let (max_w, max_h) = (window_px.0 as i32, window_px.1 as i32);
    rects
        .iter()
        .filter(|r| r.iter().all(|v| v.is_finite()))
        .take(MAX_REGION_RECTS)
        .map(|&[x, y, w, h]| {
            let l = ((x * scale).floor() as i32).clamp(0, max_w);
            let t = ((y * scale).floor() as i32).clamp(0, max_h);
            let r = (((x + w) * scale).ceil() as i32).clamp(0, max_w);
            let b = (((y + h) * scale).ceil() as i32).clamp(0, max_h);
            (l, t, r, b)
        })
        .filter(|&(l, t, r, b)| r > l && b > t)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: u32, y: u32, w: u32, h: u32) -> Cells {
        Cells { x, y, w, h }
    }

    fn item(id: &str, r: Cells) -> Item {
        Item { id: id.into(), rect: r }
    }

    /// Adds 1×1 blockers on every cell not covered by `items` and not in `keep_free`.
    fn fill(columns: u32, rows: u32, mut items: Vec<Item>, keep_free: &[(u32, u32)]) -> Vec<Item> {
        for y in 0..rows {
            for x in 0..columns {
                let cell = c(x, y, 1, 1);
                if keep_free.contains(&(x, y)) || items.iter().any(|it| intersects(&it.rect, &cell)) {
                    continue;
                }
                items.push(item(&format!("z{x}-{y}"), cell));
            }
        }
        items
    }

    #[test]
    fn move_into_empty_cells() {
        let items = vec![item("a", c(0, 0, 1, 1))];
        assert_eq!(place(4, 8, &items, 0, c(3, 7, 1, 1)).unwrap(), vec![c(3, 7, 1, 1)]);
    }

    #[test]
    fn drop_onto_a_1x1_swaps() {
        let items = vec![item("a", c(0, 0, 1, 1)), item("b", c(1, 0, 1, 1))];
        assert_eq!(place(4, 8, &items, 0, c(1, 0, 1, 1)).unwrap(), vec![c(1, 0, 1, 1), c(0, 0, 1, 1)]);
    }

    #[test]
    fn drop_a_1x1_onto_a_2x1_shoves_it_straight_below() {
        let items = vec![item("a", c(3, 7, 1, 1)), item("b", c(0, 0, 2, 1))];
        assert_eq!(place(4, 8, &items, 0, c(0, 0, 1, 1)).unwrap(), vec![c(0, 0, 1, 1), c(0, 1, 2, 1)]);
    }

    #[test]
    fn a_2x2_onto_two_1x1s_swaps_both_into_its_vacated_cells() {
        let items = vec![item("e", c(2, 6, 2, 2)), item("b", c(0, 0, 1, 1)), item("c", c(1, 0, 1, 1))];
        let out = place(4, 8, &items, 0, c(0, 0, 2, 2)).unwrap();
        assert_eq!(out, vec![c(0, 0, 2, 2), c(2, 6, 1, 1), c(3, 6, 1, 1)]);
    }

    #[test]
    fn full_grid_with_no_fit_is_refused_and_names_the_victim() {
        let items = fill(4, 8, vec![item("a", c(3, 7, 1, 1)), item("b", c(0, 0, 2, 1)), item("c", c(2, 0, 1, 1))], &[]);
        assert_eq!(place(4, 8, &items, 0, c(0, 0, 1, 1)), Err(LayoutError::GridFull { id: "b".into() }));
    }

    #[test]
    fn full_grid_adjacent_case_succeeds() {
        let items = fill(4, 8, vec![item("a", c(2, 0, 1, 1)), item("b", c(0, 0, 2, 1))], &[]);
        let out = place(4, 8, &items, 0, c(0, 0, 1, 1)).unwrap();
        assert_eq!((out[0], out[1]), (c(0, 0, 1, 1), c(1, 0, 2, 1)));
    }

    #[test]
    fn at_distance_2_straight_below_beats_below_right() {
        // V 2×1 at (2,3); its only free fits are (2,5) [0,+2] and (3,4) [+1,+1].
        let keep = [(3, 3), (2, 5), (3, 5), (3, 4), (4, 4), (6, 6)];
        let items = fill(7, 7, vec![item("m", c(6, 6, 1, 1)), item("v", c(2, 3, 2, 1))], &keep);
        let out = place(7, 7, &items, 0, c(2, 3, 1, 1)).unwrap();
        assert_eq!(out[1], c(2, 5, 2, 1));
    }

    #[test]
    fn tie_order_is_below_belowright_belowleft_right_left_above_aboveright_aboveleft() {
        let order = [(0, 2), (1, 1), (-1, 1), (2, 0), (-2, 0), (0, -2), (1, -1), (-1, -1)];
        for pair in order.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            assert!(tie_key(a.0, a.1, 5, 5) < tie_key(b.0, b.1, 5, 5), "{a:?} should beat {b:?}");
        }
    }

    #[test]
    fn target_outside_the_grid_is_refused() {
        let items = vec![item("a", c(0, 0, 1, 1))];
        assert_eq!(place(4, 8, &items, 0, c(3, 0, 2, 1)), Err(LayoutError::OutOfGrid));
        assert_eq!(place(4, 8, &items, 0, c(0, 7, 1, 2)), Err(LayoutError::OutOfGrid));
        assert_eq!(LayoutError::OutOfGrid.code(), "layout-out-of-grid");
    }

    #[test]
    fn drop_on_origin_changes_nothing() {
        let items = vec![item("a", c(1, 1, 2, 1)), item("b", c(0, 0, 1, 1))];
        assert_eq!(place(4, 8, &items, 0, c(1, 1, 2, 1)).unwrap(), vec![c(1, 1, 2, 1), c(0, 0, 1, 1)]);
    }

    #[test]
    fn resize_over_a_neighbour_shoves_it() {
        let items = vec![item("a", c(0, 0, 1, 1)), item("b", c(1, 0, 1, 1))];
        let out = place(4, 8, &items, 0, c(0, 0, 2, 1)).unwrap();
        assert_eq!(out[0], c(0, 0, 2, 1));
        assert_eq!(out[1], c(1, 1, 1, 1), "nearest free: straight below");
    }

    #[test]
    fn retry_by_descending_area_rescues_a_blocked_order() {
        // m resizes from (0,0) to cover (0,0)-(2,0), hitting s (1,0) and l (2,0)-(3,0).
        // Free: a 2-wide slot at (0,1)-(1,1), l's old (3,0) and (3,3).
        // In (row, col) order s takes (1,1) and breaks the slot, so l has no fit;
        // by area l takes the slot first and s goes to (3,0).
        let base = vec![item("m", c(0, 0, 1, 1)), item("s", c(1, 0, 1, 1)), item("l", c(2, 0, 2, 1))];
        let items = fill(4, 4, base, &[(0, 1), (1, 1), (3, 3)]);
        let out = place(4, 4, &items, 0, c(0, 0, 3, 1)).unwrap();
        assert_eq!(out[2], c(0, 1, 2, 1));
        assert_eq!(out[1], c(3, 0, 1, 1));
    }

    #[test]
    fn first_and_nearest_free() {
        let taken = [c(0, 0, 2, 1), c(2, 0, 1, 1)];
        assert_eq!(first_free(4, 8, &taken, 1, 1), Some(c(3, 0, 1, 1)));
        assert_eq!(first_free(4, 8, &taken, 2, 1), Some(c(0, 1, 2, 1)));
        assert_eq!(nearest_free(4, 8, &taken, c(0, 0, 2, 1)), Some(c(0, 1, 2, 1)));
        assert_eq!(first_free(1, 1, &[c(0, 0, 1, 1)], 1, 1), None);
    }

    /// Seeded property loop: no overlaps, inside the grid, non-victims never
    /// move, the mover lands on the target, ids and count preserved.
    #[test]
    fn property_place_keeps_the_invariants() {
        let mut seed: u64 = 0x5eed_1234;
        let mut rnd = |n: u32| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) % u64::from(n.max(1))) as u32
        };
        let mut checked = 0;
        for _ in 0..3000 {
            let (cols, rows) = (2 + rnd(6), 2 + rnd(8));
            let mut rects: Vec<Cells> = Vec::new();
            for _ in 0..rnd(cols * rows) {
                let (w, h) = (1 + rnd(2), 1 + rnd(2));
                let r = c(rnd(cols), rnd(rows), w, h);
                if inside(&r, cols, rows) && rects.iter().all(|o| !intersects(o, &r)) {
                    rects.push(r);
                }
            }
            if rects.is_empty() {
                continue;
            }
            let items: Vec<Item> = rects.iter().enumerate().map(|(i, r)| item(&format!("b{i}"), *r)).collect();
            let mover = rnd(items.len() as u32) as usize;
            let target = c(rnd(cols), rnd(rows), 1 + rnd(3), 1 + rnd(3));
            let victims: Vec<usize> = (0..items.len()).filter(|&i| i != mover && intersects(&items[i].rect, &target)).collect();
            match place(cols, rows, &items, mover, target) {
                Err(LayoutError::OutOfGrid) => assert!(!inside(&target, cols, rows)),
                Err(LayoutError::GridFull { id }) => assert!(victims.iter().any(|&v| items[v].id == id)),
                Ok(out) => {
                    checked += 1;
                    assert_eq!(out.len(), items.len());
                    assert_eq!(out[mover], target);
                    for (i, r) in out.iter().enumerate() {
                        assert!(inside(r, cols, rows), "{r:?} outside {cols}x{rows}");
                        assert_eq!((r.w, r.h), if i == mover { (target.w, target.h) } else { (items[i].rect.w, items[i].rect.h) });
                        if i != mover && !victims.contains(&i) {
                            assert_eq!(*r, items[i].rect, "non-victim moved");
                        }
                        for (j, o) in out.iter().enumerate().skip(i + 1) {
                            assert!(!intersects(r, o), "{i} {r:?} overlaps {j} {o:?}");
                        }
                    }
                }
            }
        }
        assert!(checked > 500, "only {checked} successful placements exercised");
    }

    #[test]
    fn region_rects_round_outward_at_common_scales() {
        let rects = [[10.0, 10.0, 42.0, 78.0], [60.3, 0.0, 0.4, 0.4]];
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let px = region_rects(&rects, scale, (2000, 4000));
            let (l, t, r, b) = px[0];
            assert!(f64::from(l) <= 10.0 * scale && f64::from(t) <= 10.0 * scale, "scale {scale}");
            assert!(f64::from(r) >= 52.0 * scale && f64::from(b) >= 88.0 * scale, "scale {scale}");
            assert!(f64::from(r - l) - 42.0 * scale < 2.0, "never more than a pixel of growth per edge");
            // Sub-pixel rect still covers at least one device pixel.
            assert_eq!(px.len(), 2, "scale {scale}");
        }
        assert_eq!(region_rects(&[[0.0, 0.0, 10.0, 10.0]], 1.5, (2000, 4000)), vec![(0, 0, 15, 15)]);
    }

    #[test]
    fn region_rects_clamp_cap_and_drop_bad_input() {
        let px = region_rects(&[[-5.0, -5.0, 1000.0, 1000.0]], 1.0, (300, 400));
        assert_eq!(px, vec![(0, 0, 300, 400)]);
        let many = vec![[0.0, 0.0, 1.0, 1.0]; 500];
        assert_eq!(region_rects(&many, 1.0, (10, 10)).len(), MAX_REGION_RECTS);
        assert!(region_rects(&[[f64::NAN, 0.0, 1.0, 1.0], [500.0, 500.0, 5.0, 5.0]], 1.0, (100, 100)).is_empty());
    }
}
