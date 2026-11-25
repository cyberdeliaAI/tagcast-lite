//! Scroll intent belongs to each queue view, not to the playback queue.
use eframe::egui::{self, Ui};

#[derive(Default)]
pub(super) struct QueueScroll {
    pub browsing: bool,
    pub offset: f32,
    pub current: Option<usize>,
}

impl QueueScroll {
    pub fn resume(&mut self) {
        *self = Self::default();
    }

    pub fn input(&mut self, ui: &Ui, rows: usize, row_height: f32) {
        let rect = ui.available_rect_before_wrap();
        if rows as f32 * row_height <= rect.height() || !ui.rect_contains_pointer(rect) {
            return;
        }
        // Observe the wheel before ScrollArea consumes it. This also covers trackpads.
        if ui.input(|i| {
            i.smooth_scroll_delta().y != 0.0
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::MouseWheel { delta, .. } if delta.y != 0.0))
                || (i.pointer.primary_down()
                    && i.pointer.press_origin().is_some_and(|p| {
                        rect.contains(p)
                            && (i.pointer.is_decidedly_dragging()
                                || p.x >= rect.right() - ui.spacing().scroll.bar_width)
                    }))
        }) {
            self.browsing = true;
        }
    }

    pub fn follow(&mut self, now: Option<usize>, height: f32, row: f32) -> Option<f32> {
        if self.browsing || self.current == now {
            return None;
        }
        let first = self.current.is_none();
        self.current = now;
        let y = now? as f32 * row;
        let visible = y >= self.offset && y + row <= self.offset + height;
        (first || !visible).then_some((y - 2.0 * row).max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_browsing_survives_track_changes_until_resume() {
        let mut scroll = QueueScroll::default();
        assert_eq!(scroll.follow(Some(5), 340.0, 34.0), Some(102.0));
        scroll.offset = 102.0;
        assert_eq!(scroll.follow(Some(6), 340.0, 34.0), None);
        scroll.browsing = true;
        scroll.offset = 750.0;
        assert_eq!(scroll.follow(Some(7), 340.0, 34.0), None);
        scroll.resume();
        assert_eq!(scroll.follow(Some(7), 340.0, 34.0), Some(170.0));
    }
}
