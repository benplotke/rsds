//! Geometry helpers for arranging outputs in the global layout.

/// An axis-aligned rectangle in layout pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// True if the interiors intersect (touching edges do not count).
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn at(&self, x: i32, y: i32) -> Rect {
        Rect { x, y, ..*self }
    }

    /// True if `o`'s left edge lies along our right edge for a non-empty
    /// segment.
    fn touches_right(&self, o: &Rect) -> bool {
        o.x == self.right() && o.y < self.bottom() && self.y < o.bottom()
    }

    /// True if `o`'s top edge lies along our bottom edge for a non-empty
    /// segment.
    fn touches_bottom(&self, o: &Rect) -> bool {
        o.y == self.bottom() && o.x < self.right() && self.x < o.right()
    }

    /// True if the two rects share an edge segment (not just a corner).
    pub fn adjacent(&self, o: &Rect) -> bool {
        self.touches_right(o) || o.touches_right(self) || self.touches_bottom(o) || o.touches_bottom(self)
    }
}

/// True if no two rects overlap and every rect is connected to every other
/// through shared edges.
pub fn is_valid(rects: &[Rect]) -> bool {
    for (i, a) in rects.iter().enumerate() {
        if rects[i + 1..].iter().any(|b| a.overlaps(b)) {
            return false;
        }
    }
    components(rects).len() <= 1
}

/// Group `rects` into sets connected through shared edges.
/// Returns indices into `rects`.
fn components(rects: &[Rect]) -> Vec<Vec<usize>> {
    let mut group = vec![usize::MAX; rects.len()];
    let mut out: Vec<Vec<usize>> = Vec::new();
    for start in 0..rects.len() {
        if group[start] != usize::MAX {
            continue;
        }
        let g = out.len();
        let mut members = vec![start];
        group[start] = g;
        let mut k = 0;
        while k < members.len() {
            let a = rects[members[k]];
            for j in 0..rects.len() {
                if group[j] == usize::MAX && a.adjacent(&rects[j]) {
                    group[j] = g;
                    members.push(j);
                }
            }
            k += 1;
        }
        out.push(members);
    }
    out
}

/// Move `others` to follow a monitor that changed from `old` to `new`
/// (same top-left corner). Groups of monitors attached to its right edge
/// move right by the change in width, and groups attached to its bottom edge
/// move down by the change in height, so growing and then shrinking a
/// monitor returns everything to where it was.
pub fn follow_resize(old: Rect, new: Rect, others: &mut [Rect]) {
    let dw = new.right() - old.right();
    let dh = new.bottom() - old.bottom();
    for members in components(others) {
        let right = members.iter().any(|&i| old.touches_right(&others[i]));
        let below = members.iter().any(|&i| old.touches_bottom(&others[i]));
        for &i in &members {
            if right {
                others[i].x += dw;
            }
            if below {
                others[i].y += dh;
            }
        }
    }
}

/// Snap a rectangle being dropped at `moving` to a valid position.
///
/// The result shares an edge segment (not just a corner) with at least one
/// of `others`, does not overlap any of them, and is the closest such
/// position to where it was dropped. Edges within `threshold` of another
/// output's parallel edge are aligned with it.
pub fn snap(moving: Rect, others: &[Rect], threshold: i32) -> Rect {
    snap_inner(moving, others, threshold, false)
}

/// Like [`snap`], but when `full_edge` is set, a rect that has to slide along
/// a neighbour's edge goes far enough to share the whole of the shorter
/// edge, rather than stopping at a sliver.
fn snap_inner(moving: Rect, others: &[Rect], threshold: i32, full_edge: bool) -> Rect {
    if others.is_empty() {
        return moving.at(0, 0);
    }

    let mut best: Option<(i64, Rect)> = None;
    let mut consider = |r: Rect| {
        if others.iter().any(|o| o.overlaps(&r)) {
            return;
        }
        let dx = (r.x - moving.x) as i64;
        let dy = (r.y - moving.y) as i64;
        let d = dx * dx + dy * dy;
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, r));
        }
    };

    for o in others {
        // Valid range for the free axis so that the shared edge is non-empty
        // (or, with `full_edge`, as long as the shorter of the two edges).
        let range = |start: i32, end: i32, len: i32| {
            if full_edge {
                let (a, b) = (start, end - len);
                (a.min(b), a.max(b))
            } else {
                (start - len + 1, end - 1)
            }
        };
        let y_range = range(o.y, o.bottom(), moving.h);
        let x_range = range(o.x, o.right(), moving.w);

        for x in [o.right(), o.x - moving.w] {
            let y = align(moving.y, moving.h, others, threshold, false, y_range);
            consider(moving.at(x, y));
        }
        for y in [o.bottom(), o.y - moving.h] {
            let x = align(moving.x, moving.w, others, threshold, true, x_range);
            consider(moving.at(x, y));
        }
    }

    match best {
        Some((_, r)) => r,
        // Everything around is crowded: put it to the right of everything.
        None => {
            let x = others.iter().map(Rect::right).max().unwrap_or(0);
            let y = others.iter().map(|o| o.y).min().unwrap_or(0);
            moving.at(x, y)
        }
    }
}

/// Clamp a coordinate into `range`, then align its start or end edge with
/// a parallel edge of another rect if one is within `threshold`.
fn align(pos: i32, len: i32, others: &[Rect], threshold: i32, horizontal: bool, range: (i32, i32)) -> i32 {
    let pos = pos.clamp(range.0, range.1);
    let mut best: Option<(i32, i32)> = None;
    for o in others {
        let (start, end) = if horizontal { (o.x, o.right()) } else { (o.y, o.bottom()) };
        // Candidate positions: our start or end aligned with their start or end.
        for cand in [start, end, start - len, end - len] {
            let d = (cand - pos).abs();
            if d <= threshold
                && (range.0..=range.1).contains(&cand)
                && best.is_none_or(|(bd, _)| d < bd)
            {
                best = Some((d, cand));
            }
        }
    }
    best.map_or(pos, |(_, c)| c)
}

/// Rearrange `rects` so none overlap and none float detached from the
/// group. `fixed` (if any) stays where it is; the others are placed in order
/// of distance from `centre`. Each one is left alone if it already shares an
/// edge with one placed before it, and otherwise snapped to them.
/// Returns the new rects in the same order as `rects`.
pub fn compact(fixed: Option<Rect>, centre: (i32, i32), rects: &[Rect]) -> Vec<Rect> {
    let mut order: Vec<usize> = (0..rects.len()).collect();
    order.sort_by_key(|&i| {
        let r = rects[i];
        let dx = (r.x + r.w / 2 - centre.0) as i64;
        let dy = (r.y + r.h / 2 - centre.1) as i64;
        dx * dx + dy * dy
    });
    let mut placed: Vec<Rect> = fixed.into_iter().collect();
    let mut out = rects.to_vec();
    for i in order {
        let r = rects[i];
        let attached = placed.iter().any(|p| p.adjacent(&r)) && !placed.iter().any(|p| p.overlaps(&r));
        let new = if placed.is_empty() || attached { r } else { snap_inner(r, &placed, 0, true) };
        out[i] = new;
        placed.push(new);
    }
    out
}

/// Position for a new rect to the right of all of `existing`,
/// top-aligned with the rightmost one.
pub fn place_right(existing: &[Rect]) -> (i32, i32) {
    match existing.iter().max_by_key(|r| (r.right(), -r.y)) {
        Some(r) => (r.right(), r.y),
        None => (0, 0),
    }
}

/// Offset that moves the top-left of the bounding box of `rects` to (0, 0).
pub fn normalize_offset(rects: &[Rect]) -> (i32, i32) {
    let x = rects.iter().map(|r| r.x).min().unwrap_or(0);
    let y = rects.iter().map(|r| r.y).min().unwrap_or(0);
    (-x, -y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: (i32, i32) = (1920, 1080);

    fn r(x: i32, y: i32, (w, h): (i32, i32)) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn snaps_gap_closed() {
        let others = [r(0, 0, FHD)];
        let got = snap(r(2000, 0, FHD), &others, 50);
        assert_eq!(got, r(1920, 0, FHD));
    }

    #[test]
    fn pushes_out_of_overlap() {
        let others = [r(0, 0, FHD)];
        let got = snap(r(1800, 30, FHD), &others, 50);
        assert_eq!(got, r(1920, 0, FHD));
    }

    #[test]
    fn keeps_offset_outside_threshold() {
        let others = [r(0, 0, FHD)];
        let got = snap(r(1920, 300, FHD), &others, 50);
        assert_eq!(got, r(1920, 300, FHD));
    }

    #[test]
    fn aligns_bottom_edges() {
        let others = [r(0, 0, (3840, 2160))];
        let got = snap(r(3840, 1100, FHD), &others, 50);
        assert_eq!(got, r(3840, 1080, FHD));
    }

    #[test]
    fn corner_only_is_not_adjacent() {
        let others = [r(0, 0, FHD)];
        // Diagonal from the first monitor: must slide to share an edge.
        let got = snap(r(1920, 1080, FHD), &others, 0);
        assert!(got == r(1920, 1079, FHD) || got == r(1919, 1080, FHD), "{got:?}");
    }

    #[test]
    fn snaps_below() {
        let others = [r(0, 0, FHD)];
        let got = snap(r(10, 1200, FHD), &others, 50);
        assert_eq!(got, r(0, 1080, FHD));
    }

    #[test]
    fn compact_closes_gap() {
        let got = compact(None, (0, 0), &[r(0, 0, FHD), r(3840, 0, FHD)]);
        assert_eq!(got, vec![r(0, 0, FHD), r(1920, 0, FHD)]);
    }

    #[test]
    fn place_right_of_rightmost() {
        assert_eq!(place_right(&[r(0, 0, FHD), r(1920, 200, FHD)]), (3840, 200));
        assert_eq!(place_right(&[]), (0, 0));
    }

    #[test]
    fn avoids_overlapping_third_monitor() {
        let others = [r(0, 0, FHD), r(1920, 0, FHD)];
        let got = snap(r(1900, 10, FHD), &others, 50);
        assert!(!others.iter().any(|o| o.overlaps(&got)), "{got:?}");
    }

    const QHD: (i32, i32) = (2560, 1440);
    const UHD: (i32, i32) = (3840, 2160);

    #[test]
    fn neighbours_follow_resize_and_return() {
        // DP-7 right of DP-6, eDP-1 below it.
        let start = [r(2560, 0, FHD), r(0, 1440, FHD)];
        let mut others = start;
        follow_resize(r(0, 0, QHD), r(0, 0, UHD), &mut others);
        assert_eq!(others, [r(3840, 0, FHD), r(0, 2160, FHD)]);
        follow_resize(r(0, 0, UHD), r(0, 0, QHD), &mut others);
        assert_eq!(others, start);
    }

    #[test]
    fn resize_leaves_unrelated_groups() {
        // L left of A, B below L (not below A), R right of A, D below R.
        let mut others = [r(-1920, 0, FHD), r(-1920, 1080, FHD), r(1920, 0, FHD), r(1920, 1080, FHD)];
        follow_resize(r(0, 0, FHD), r(0, 0, QHD), &mut others);
        assert_eq!(others, [r(-1920, 0, FHD), r(-1920, 1080, FHD), r(2560, 0, FHD), r(2560, 1080, FHD)]);
        let mut all = vec![r(0, 0, QHD)];
        all.extend(others);
        assert!(is_valid(&all));
    }

    #[test]
    fn detects_corner_only_and_overlap() {
        assert!(is_valid(&[r(0, 0, FHD), r(1920, 500, FHD)]));
        assert!(!is_valid(&[r(0, 0, FHD), r(1920, 1080, FHD)]));
        assert!(!is_valid(&[r(0, 0, FHD), r(1000, 0, FHD)]));
    }

    #[test]
    fn compact_aligns_instead_of_sliver() {
        // Was above a monitor that shrank; now only touches its corner, so it
        // moves to the nearest fully shared edge rather than a 1px sliver.
        let got = compact(Some(r(0, 1080, QHD)), (1280, 1800), &[r(3000, 0, FHD)]);
        assert_eq!(got, vec![r(2560, 1080, FHD)]);
    }
}
