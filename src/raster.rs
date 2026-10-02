// ABOUTME: Internal software rasterizer: colored dots and depth resolve into terminal cells.
// ABOUTME: This module does not read input, write output, or own the terminal.
use std::fmt::Write;

pub type Rgb = [u8; 3];
const DOT_BITS: [u8; 8] = [1, 8, 2, 16, 4, 32, 64, 128];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub glyph: char,
    pub color: Rgb,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            glyph: ' ',
            color: [0; 3],
        }
    }
}

#[derive(Clone, Copy)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    /// Smaller positive values are nearer the camera.
    pub depth: f32,
}

pub struct Canvas {
    width: usize,
    height: usize,
    depths: Vec<f32>,
    colors: Vec<Rgb>,
    cells: Vec<Cell>,
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            depths: vec![f32::INFINITY; width * height * 8],
            colors: vec![[0; 3]; width * height * 8],
            cells: vec![Cell::default(); width * height],
        }
    }

    pub fn clear(&mut self) {
        self.depths.fill(f32::INFINITY);
    }

    pub fn dot(&mut self, x: i32, y: i32, depth: f32, color: Rgb) {
        if x < 0
            || y < 0
            || x as usize >= self.width * 2
            || y as usize >= self.height * 4
            || !depth.is_finite()
            || depth <= 0.0
        {
            return;
        }
        let index = y as usize * self.width * 2 + x as usize;
        if depth < self.depths[index] {
            self.depths[index] = depth;
            self.colors[index] = color;
        }
    }

    pub fn line(&mut self, a: Point, b: Point, color: Rgb) {
        let steps = (b.x - a.x).abs().max((b.y - a.y).abs()).ceil() as usize;
        // Bounds also limit the work for an accidentally enormous offscreen line.
        if steps > (self.width + self.height) * 16 {
            return;
        }
        for step in 0..=steps {
            let t = step as f32 / steps.max(1) as f32;
            self.dot(
                (a.x + (b.x - a.x) * t).round() as i32,
                (a.y + (b.y - a.y) * t).round() as i32,
                a.depth + (b.depth - a.depth) * t,
                color,
            );
        }
    }

    /// Fill a projected triangle. Interpolate reciprocal depth for perspective occlusion.
    pub fn triangle(&mut self, vertices: [Point; 3], color: Rgb) {
        let [a, b, c] = vertices;
        if vertices
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || !p.depth.is_finite() || p.depth <= 0.0)
        {
            return;
        }
        let area = edge(a, b, c.x, c.y);
        if area.abs() < 0.0001 {
            return;
        }
        let left = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
        let top = a.y.min(b.y).min(c.y).floor().max(0.0) as usize;
        let right = (a.x.max(b.x).max(c.x).ceil().max(0.0) as usize).min(self.width * 2);
        let bottom = (a.y.max(b.y).max(c.y).ceil().max(0.0) as usize).min(self.height * 4);
        for y in top..bottom {
            for x in left..right {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let wa = edge(b, c, px, py) / area;
                let wb = edge(c, a, px, py) / area;
                let wc = 1.0 - wa - wb;
                if wa >= -0.0001 && wb >= -0.0001 && wc >= -0.0001 {
                    let depth = 1.0 / (wa / a.depth + wb / b.depth + wc / c.depth);
                    self.dot(x as i32, y as i32, depth, color);
                }
            }
        }
    }

    /// Each braille cell has one foreground color: average the colors of its visible dots.
    pub fn resolve(&mut self) -> &[Cell] {
        for row in 0..self.height {
            for column in 0..self.width {
                let mut bits = 0;
                let mut sum = [0_u32; 3];
                let mut count = 0;
                for (dot, bit) in DOT_BITS.iter().enumerate() {
                    let index = (row * 4 + dot / 2) * self.width * 2 + column * 2 + dot % 2;
                    if self.depths[index].is_finite() {
                        bits |= bit;
                        count += 1;
                        for (total, channel) in sum.iter_mut().zip(self.colors[index]) {
                            *total += u32::from(channel);
                        }
                    }
                }
                self.cells[row * self.width + column] = if count == 0 {
                    Cell::default()
                } else {
                    Cell {
                        glyph: char::from_u32(0x2800 + u32::from(bits)).unwrap(),
                        color: sum.map(|channel| (channel / count) as u8),
                    }
                };
            }
        }
        &self.cells
    }
}

fn edge(a: Point, b: Point, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

/// Encode changed runs within an explicit rectangle. The caller controls output and synchronization.
pub struct Encoder {
    width: usize,
    previous: Vec<Cell>,
    valid: bool,
    truecolor: bool,
}

impl Encoder {
    pub fn new(width: usize, height: usize, truecolor: bool) -> Self {
        Self {
            width,
            previous: vec![Cell::default(); width * height],
            valid: false,
            truecolor,
        }
    }

    pub fn encode(&mut self, cells: &[Cell], origin: (u16, u16), output: &mut String) {
        assert_eq!(cells.len(), self.previous.len());
        let mut position = 0;
        let mut color = None;
        while position < cells.len() {
            if self.valid && cells[position] == self.previous[position] {
                position += 1;
                continue;
            }
            let row = position / self.width;
            let _ = write!(
                output,
                "\x1b[{};{}H",
                usize::from(origin.1) + row + 1,
                usize::from(origin.0) + position % self.width + 1
            );
            while position < cells.len()
                && position / self.width == row
                && (!self.valid || cells[position] != self.previous[position])
            {
                let cell = cells[position];
                if cell.glyph != ' ' && color != Some(cell.color) {
                    if self.truecolor {
                        let [r, g, b] = cell.color;
                        let _ = write!(output, "\x1b[38;2;{r};{g};{b}m");
                    } else {
                        let _ = write!(output, "\x1b[38;5;{}m", indexed(cell.color));
                    }
                    color = Some(cell.color);
                }
                output.push(cell.glyph);
                position += 1;
            }
        }
        if !output.is_empty() {
            output.push_str("\x1b[0m");
        }
        self.previous.copy_from_slice(cells);
        self.valid = true;
    }
}

fn indexed(rgb: Rgb) -> u8 {
    let [r, g, b] = rgb.map(|v| ((u16::from(v) * 5 + 127) / 255) as u8);
    16 + r * 36 + g * 6 + b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braille_dots_follow_unicode_order_and_average_visible_colors() {
        let mut canvas = Canvas::new(1, 1);
        for (index, bit) in DOT_BITS.iter().enumerate() {
            canvas.clear();
            canvas.dot((index % 2) as i32, (index / 2) as i32, 1.0, [80, 120, 160]);
            assert_eq!(canvas.resolve()[0].glyph as u32, 0x2800 + u32::from(*bit));
        }
        canvas.clear();
        canvas.dot(0, 0, 2.0, [200, 0, 0]);
        canvas.dot(0, 0, 1.0, [0, 100, 200]);
        canvas.dot(1, 0, 1.0, [100, 0, 0]);
        assert_eq!(
            canvas.resolve()[0],
            Cell {
                glyph: '⠉',
                color: [50, 50, 100]
            }
        );
    }

    #[test]
    fn triangles_clip_and_nearer_surfaces_win_in_either_order() {
        let triangle = |depth| {
            [
                Point {
                    x: -10.0,
                    y: -10.0,
                    depth,
                },
                Point {
                    x: 50.0,
                    y: -10.0,
                    depth,
                },
                Point {
                    x: -10.0,
                    y: 50.0,
                    depth,
                },
            ]
        };
        let mut canvas = Canvas::new(2, 2);
        for reverse in [false, true] {
            canvas.clear();
            for depth in if reverse { [1.0, 2.0] } else { [2.0, 1.0] } {
                canvas.triangle(
                    triangle(depth),
                    if depth == 1.0 {
                        [20, 80, 100]
                    } else {
                        [200; 3]
                    },
                );
            }
            assert!(
                canvas
                    .resolve()
                    .iter()
                    .all(|c| c.glyph == '⣿' && c.color == [20, 80, 100])
            );
        }
        canvas.clear();
        canvas.dot(-1, 0, 1.0, [255; 3]);
        canvas.dot(4, 0, 1.0, [255; 3]);
        canvas.dot(0, 8, 1.0, [255; 3]);
        canvas.dot(0, 0, f32::NAN, [255; 3]);
        assert!(canvas.resolve().iter().all(|c| *c == Cell::default()));
    }

    #[test]
    fn unchanged_frames_emit_nothing_and_erased_cells_emit_spaces() {
        let mut encoder = Encoder::new(2, 1, false);
        let mut cells = [
            Cell {
                glyph: '⣿',
                color: [0, 255, 255],
            },
            Cell::default(),
        ];
        let mut output = String::new();
        encoder.encode(&cells, (3, 4), &mut output);
        assert!(output.starts_with("\x1b[5;4H\x1b[38;5;51m"));
        output.clear();
        encoder.encode(&cells, (3, 4), &mut output);
        assert!(output.is_empty());
        cells[0] = Cell::default();
        encoder.encode(&cells, (3, 4), &mut output);
        assert_eq!(output, "\x1b[5;4H \x1b[0m");
    }
}
