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
}

/// Snap a rectangle being dropped at `moving` to a valid position.
///
/// The result shares an edge segment (not just a corner) with at least one
/// of `others`, does not overlap any of them, and is the closest such
/// position to where it was dropped. Edges within `threshold` of another
/// output's parallel edge are aligned with it.
pub fn snap(moving: Rect, others: &[Rect], threshold: i32) -> Rect {
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
        // Valid range for the free axis so that the shared edge is non-empty.
        let y_range = (o.y - moving.h + 1, o.bottom() - 1);
        let x_range = (o.x - moving.w + 1, o.right() - 1);

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
    fn avoids_overlapping_third_monitor() {
        let others = [r(0, 0, FHD), r(1920, 0, FHD)];
        let got = snap(r(1900, 10, FHD), &others, 50);
        assert!(!others.iter().any(|o| o.overlaps(&got)), "{got:?}");
    }
}
