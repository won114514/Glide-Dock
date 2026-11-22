//! Per-pixel Win32 hit testing comes from the layered DIB. Its one-bit coverage
//! is independent of the DirectComposition visual layered over it.

use dock_platform_windows::{LayeredSurface, Result};
use dock_render::{DockFrame, HitShape};
use windows::Win32::Foundation::RECT;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Span(i32, i32);

#[derive(Default)]
pub(super) struct InputMask {
    displayed: Vec<Vec<Span>>,
    next: Vec<Vec<Span>>,
}

impl InputMask {
    pub(super) fn present(
        &mut self,
        surface: &mut LayeredSurface,
        frame: &DockFrame,
    ) -> Result<()> {
        let width = frame.width as i32;
        let height = frame.height as i32;
        build_rows(&mut self.next, width, height, &frame.hit_shapes);
        surface.update_input_mask(frame.width, frame.height, |pixels, rebuild| {
            let mut dirty = None;
            for (y, spans) in self.next.iter().enumerate() {
                let old: &[Span] = if rebuild {
                    &[][..]
                } else {
                    self.displayed.get(y).map_or(&[], Vec::as_slice)
                };
                if old == spans {
                    continue;
                }
                let row = &mut pixels[y * width as usize..][..width as usize];
                let mut paint = |span: Span, hit: bool| {
                    // Minimum nonzero alpha. The DComp layer provides the
                    // artwork, including pixels outside these spans.
                    row[span.0 as usize..span.1 as usize].fill(if hit { 0x0100_0000 } else { 0 });
                    extend_dirty(&mut dirty, span, y as i32);
                };
                difference(old, spans, |span| paint(span, false));
                difference(spans, old, |span| paint(span, true));
            }
            dirty
        })?;
        std::mem::swap(&mut self.displayed, &mut self.next);
        Ok(())
    }
}

fn build_rows(rows: &mut Vec<Vec<Span>>, width: i32, height: i32, shapes: &[HitShape]) {
    rows.resize_with(height as usize, Vec::new);
    rows.iter_mut().for_each(Vec::clear);
    for shape in shapes {
        let top = (shape.rect.top().ceil() as i32).clamp(0, height);
        let bottom = (shape.rect.bottom().ceil() as i32).clamp(top, height);
        for y in top..bottom {
            if let Some((left, right)) = shape.span(y) {
                let left = left.max(0);
                let right = right.min(width);
                if left < right {
                    rows[y as usize].push(Span(left, right));
                }
            }
        }
    }
    for spans in rows {
        spans.sort_unstable();
        let mut merged = 0;
        for i in 0..spans.len() {
            let span = spans[i];
            if merged > 0 && span.0 <= spans[merged - 1].1 {
                spans[merged - 1].1 = spans[merged - 1].1.max(span.1);
            } else {
                spans[merged] = span;
                merged += 1;
            }
        }
        spans.truncate(merged);
    }
}

// Visit just the pixels in `source` that are absent from `subtract`.
fn difference(source: &[Span], subtract: &[Span], mut emit: impl FnMut(Span)) {
    let mut index = 0;
    for &Span(mut left, right) in source {
        while let Some(&Span(cut_left, cut_right)) = subtract.get(index) {
            if cut_left >= right {
                break;
            }
            if left < cut_left {
                emit(Span(left, cut_left));
            }
            left = left.max(cut_right);
            if left >= right {
                break;
            }
            index += 1;
        }
        if left < right {
            emit(Span(left, right));
        }
    }
}

fn extend_dirty(dirty: &mut Option<RECT>, span: Span, y: i32) {
    if let Some(dirty) = dirty {
        dirty.left = dirty.left.min(span.0);
        dirty.right = dirty.right.max(span.1);
        dirty.bottom = y + 1;
    } else {
        *dirty = Some(RECT {
            left: span.0,
            top: y,
            right: span.1,
            bottom: y + 1,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dock_core::Rect;

    #[test]
    fn mask_clips_and_unions_rounded_background_and_icon() {
        let shapes = [
            HitShape {
                rect: Rect::new(2.0, 2.0, 12.0, 12.0),
                radius: 6.0,
            },
            HitShape {
                rect: Rect::new(6.0, -1.0, 4.0, 8.0),
                radius: 0.0,
            },
        ];
        let mut rows = Vec::new();
        build_rows(&mut rows, 12, 16, &shapes);
        assert_eq!(rows[0], [Span(6, 10)]);
        assert_eq!(rows[5], [Span(3, 12)]);
        assert_eq!(rows[8], [Span(2, 12)]);
        assert!(rows[15].is_empty());
    }

    #[test]
    fn touching_shapes_do_not_repaint_unchanged_pixels() {
        let before = [Span(2, 8), Span(9, 12)];
        let after = [Span(0, 4), Span(5, 11)];
        let mut clear = Vec::new();
        let mut paint = Vec::new();
        difference(&before, &after, |span| clear.push(span));
        difference(&after, &before, |span| paint.push(span));
        assert_eq!(clear, [Span(4, 5), Span(11, 12)]);
        assert_eq!(paint, [Span(0, 2), Span(8, 9)]);
    }
}
