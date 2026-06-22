use smithay::utils::{Logical, Point, Rectangle, Size};

const CHAR_WIDTH: i32 = 6;
const CHAR_HEIGHT: i32 = 8;
const LINE_HEIGHT: i32 = 14;
const PADDING: i32 = 18;
const GAP: i32 = 14;
const MAX_WIDTH_FRACTION: f64 = 0.82;
const MAX_HEIGHT_FRACTION: f64 = 0.82;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelpEntry {
    pub(crate) keys: String,
    pub(crate) action: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelpMenu {
    entries: Vec<HelpEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelpLayout {
    pub(crate) panel: Rectangle<i32, Logical>,
    pub(crate) rows: Vec<HelpRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HelpRow {
    pub(crate) keys: String,
    pub(crate) action: String,
    pub(crate) baseline: Point<i32, Logical>,
    pub(crate) action_x: i32,
}

impl HelpMenu {
    pub(crate) fn new(mut entries: Vec<HelpEntry>) -> Self {
        entries.sort_by(|a, b| a.keys.cmp(&b.keys));
        Self { entries }
    }

    pub(crate) fn layout(&self, output_size: Size<i32, Logical>) -> Option<HelpLayout> {
        if self.entries.is_empty() || output_size.w <= 0 || output_size.h <= 0 {
            return None;
        }

        let max_width = (f64::from(output_size.w) * MAX_WIDTH_FRACTION).round() as i32;
        let max_height = (f64::from(output_size.h) * MAX_HEIGHT_FRACTION).round() as i32;
        let max_rows = ((max_height - PADDING * 2) / LINE_HEIGHT).max(1) as usize;
        let visible_entries = &self.entries[..self.entries.len().min(max_rows)];

        let key_width = visible_entries
            .iter()
            .map(|entry| text_width(&entry.keys))
            .max()
            .unwrap_or(0);
        let action_width = visible_entries
            .iter()
            .map(|entry| text_width(&entry.action))
            .max()
            .unwrap_or(0);
        let content_width = key_width + GAP + action_width;
        let panel_width = (content_width + PADDING * 2)
            .min(max_width)
            .max(PADDING * 2 + 1);
        let panel_height = (visible_entries.len() as i32 * LINE_HEIGHT + PADDING * 2)
            .min(max_height)
            .max(PADDING * 2 + LINE_HEIGHT);
        let panel = Rectangle::new(
            Point::from((
                (output_size.w - panel_width) / 2,
                (output_size.h - panel_height) / 2,
            )),
            Size::from((panel_width, panel_height)),
        );
        let key_x = panel.loc.x + PADDING;
        let action_x = (key_x + key_width + GAP).min(panel.loc.x + panel.size.w - PADDING);
        let mut rows = Vec::with_capacity(visible_entries.len());
        for (index, entry) in visible_entries.iter().enumerate() {
            rows.push(HelpRow {
                keys: entry.keys.clone(),
                action: entry.action.clone(),
                baseline: Point::from((key_x, panel.loc.y + PADDING + index as i32 * LINE_HEIGHT)),
                action_x,
            });
        }

        Some(HelpLayout { panel, rows })
    }
}

pub(crate) fn text_rects(
    text: &str,
    origin: Point<i32, Logical>,
) -> impl Iterator<Item = Rectangle<i32, Logical>> + '_ {
    text.chars().enumerate().flat_map(move |(index, ch)| {
        glyph_rects(ch).map(move |rect| {
            let offset = Point::from((origin.x + index as i32 * CHAR_WIDTH, origin.y));
            Rectangle::new(offset + rect.loc, rect.size)
        })
    })
}

fn text_width(text: &str) -> i32 {
    text.chars().count() as i32 * CHAR_WIDTH
}

fn glyph_rects(ch: char) -> impl Iterator<Item = Rectangle<i32, Logical>> {
    let bits = glyph_bits(ch);
    (0..CHAR_HEIGHT).flat_map(move |row| {
        (0..5).filter_map(move |col| {
            let mask = 1 << (4 - col);
            if bits[row as usize] & mask == 0 {
                return None;
            }
            Some(Rectangle::new(Point::from((col, row)), Size::from((1, 1))))
        })
    })
}

fn glyph_bits(ch: char) -> [u8; 8] {
    match ch.to_ascii_uppercase() {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11, 0x00],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e, 0x00],
        'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f, 0x00],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e, 0x00],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f, 0x00],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10, 0x00],
        'G' => [0x0f, 0x10, 0x10, 0x13, 0x11, 0x11, 0x0f, 0x00],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11, 0x00],
        'I' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1f, 0x00],
        'J' => [0x01, 0x01, 0x01, 0x01, 0x11, 0x11, 0x0e, 0x00],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11, 0x00],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f, 0x00],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11, 0x00],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11, 0x00],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e, 0x00],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10, 0x00],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d, 0x00],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11, 0x00],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e, 0x00],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e, 0x00],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04, 0x00],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a, 0x00],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11, 0x00],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04, 0x00],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f, 0x00],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e, 0x00],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e, 0x00],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f, 0x00],
        '3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e, 0x00],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02, 0x00],
        '5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e, 0x00],
        '6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e, 0x00],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08, 0x00],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e, 0x00],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e, 0x00],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1f, 0x04, 0x04, 0x00, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0c, 0x04, 0x08, 0x00],
        ':' => [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00, 0x00],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02, 0x00],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08, 0x00],
        '[' => [0x0e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0e, 0x00],
        ']' => [0x0e, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0e, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f, 0x00],
        ' ' => [0x00; 8],
        _ => [0x1f, 0x11, 0x02, 0x04, 0x04, 0x00, 0x04, 0x00],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_panel_and_limits_visible_rows() {
        let menu = HelpMenu::new(vec![
            HelpEntry {
                keys: "B".to_owned(),
                action: "Second".to_owned(),
            },
            HelpEntry {
                keys: "A".to_owned(),
                action: "First".to_owned(),
            },
        ]);
        let layout = menu.layout(Size::from((800, 600))).expect("layout");
        assert_eq!(layout.rows[0].keys, "A");
        assert_eq!(layout.rows[1].keys, "B");
        assert!(layout.panel.loc.x > 0);
        assert!(layout.panel.loc.y > 0);
    }

    #[test]
    fn text_rects_emit_pixels_for_known_glyphs() {
        let rects = text_rects("A/", Point::from((10, 20))).collect::<Vec<_>>();
        assert!(rects.iter().any(|rect| rect.loc == Point::from((11, 20))));
        assert!(rects.iter().any(|rect| rect.loc == Point::from((16, 20))));
    }
}
