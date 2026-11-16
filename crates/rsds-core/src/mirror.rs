//! Duplicating a monitor. A duplicate is an output placed at the same
//! position as its source and scaled to show the source's area, keeping its
//! own resolution. (wlroots compositors such as labwc draw overlapping
//! outputs with the same content.) If the aspect ratios differ, the
//! duplicate shows a little more than its source along one edge.

use crate::{Mode, Transform, layout::Rect};

/// How far, in layout pixels, a duplicate's size may be off its source's
/// because its scale is rounded.
const TOLERANCE: i32 = 3;

/// Scale at which an output with `mode` and `transform` shows the area of
/// `source` (a layout rect): the largest at which it still covers it, in the
/// 1/256 steps the output-management protocol carries.
pub fn scale_to_cover(mode: &Mode, transform: Transform, source: Rect) -> f64 {
    let (w, h) = if transform.is_rotated() { (mode.height, mode.width) } else { (mode.width, mode.height) };
    let s = (w as f64 / source.w.max(1) as f64).min(h as f64 / source.h.max(1) as f64);
    ((s * 256.0).round() / 256.0).max(1.0 / 256.0)
}

/// Whether `dup` is laid out as a duplicate of `source`: at the same
/// position, covering it, and the same size along at least one axis.
pub fn is_duplicate_of(dup: Rect, source: Rect) -> bool {
    dup.x == source.x
        && dup.y == source.y
        && dup.w >= source.w - TOLERANCE
        && dup.h >= source.h - TOLERANCE
        && ((dup.w - source.w).abs() <= TOLERANCE || (dup.h - source.h).abs() <= TOLERANCE)
}

/// Which of `rects` (None for outputs that are off) duplicate another: for
/// each, the index of its source. Outputs at the same position form a group
/// with one source, which every other member must duplicate. Where more
/// than one member could be the source (they are the same size), the one
/// `hint` names for a member is used, else the first.
pub fn infer(rects: &[Option<Rect>], hint: &[Option<usize>]) -> Vec<Option<usize>> {
    let n = rects.len();
    let mut out = vec![None; n];
    let mut seen = vec![false; n];
    for i in 0..n {
        let Some(a) = rects[i] else { continue };
        if seen[i] {
            continue;
        }
        let group: Vec<usize> = (0..n).filter(|&j| rects[j].is_some_and(|b| (b.x, b.y) == (a.x, a.y))).collect();
        for &j in &group {
            seen[j] = true;
        }
        if group.len() < 2 {
            continue;
        }
        let rect = |j: usize| rects[j].unwrap_or(a);
        let source_of_all = |s: &usize| group.iter().all(|&j| j == *s || is_duplicate_of(rect(j), rect(*s)));
        let hinted = group.iter().find_map(|&j| hint.get(j).copied().flatten()).filter(|s| group.contains(s));
        let Some(source) = hinted.filter(source_of_all).or_else(|| group.iter().copied().find(source_of_all))
        else {
            continue;
        };
        for &j in group.iter().filter(|&&j| j != source) {
            out[j] = Some(source);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: i32, height: i32) -> Mode {
        Mode { width, height, refresh_mhz: 60000, preferred: false }
    }

    #[test]
    fn scales_to_source_size() {
        let fhd = Rect::new(0, 0, 1920, 1080);
        // As tested on labwc: 2560x1440 duplicating 1920x1080 at 341/256.
        assert_eq!(scale_to_cover(&mode(2560, 1440), Transform::Normal, fhd), 341.0 / 256.0);
        assert_eq!(scale_to_cover(&mode(1920, 1080), Transform::Normal, fhd), 1.0);
        // 16:10 duplicating 16:9: matches the width, shows more below.
        let s = scale_to_cover(&mode(1920, 1200), Transform::Normal, fhd);
        assert_eq!(s, 1.0);
        // Rotated: its height is 1920 px wide.
        let tall = Rect::new(0, 0, 1080, 1920);
        assert_eq!(scale_to_cover(&mode(1920, 1080), Transform::Rotate90, tall), 1.0);
    }

    #[test]
    fn infers_source_and_duplicates() {
        let rects = [
            Some(Rect::new(0, 0, 1920, 1080)),
            Some(Rect::new(1920, 0, 1920, 1080)),
            // Scaled 2560x1440 duplicate of the first, off by rounding.
            Some(Rect::new(0, 0, 1922, 1081)),
            None,
        ];
        assert_eq!(infer(&rects, &[]), vec![None, None, Some(0), None]);
    }

    #[test]
    fn larger_one_is_the_duplicate() {
        // A 16:10 duplicate shows more than its 16:9 source below it.
        let rects = [Some(Rect::new(0, 0, 1920, 1200)), Some(Rect::new(0, 0, 1920, 1080))];
        assert_eq!(infer(&rects, &[]), vec![Some(1), None]);
    }

    #[test]
    fn same_size_uses_hint() {
        let rects = [Some(Rect::new(0, 0, 1920, 1080)), Some(Rect::new(0, 0, 1920, 1080))];
        assert_eq!(infer(&rects, &[]), vec![None, Some(0)]);
        assert_eq!(infer(&rects, &[Some(1), None]), vec![Some(1), None]);
    }

    #[test]
    fn unrelated_overlap_is_not_a_duplicate() {
        let rects = [Some(Rect::new(0, 0, 1920, 1080)), Some(Rect::new(0, 0, 1280, 1024))];
        assert_eq!(infer(&rects, &[]), vec![None, None]);
    }
}
