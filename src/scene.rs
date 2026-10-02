// ABOUTME: Deterministic Beam choreography: a voxel workspace travels between two destinations.
// ABOUTME: Effect calculations are independent of terminal encoding, timing, and input.
use crate::raster::{Canvas, Point, Rgb};

/// Draw the timeline at `progress` (0 to 1). `seconds` turns the scattered cloud, so a long
/// transfer stays in motion while its progress holds still.
pub fn render(canvas: &mut Canvas, width: usize, height: usize, progress: f32, seconds: f32) {
    canvas.clear();
    let t = progress.clamp(0.0, 1.0);
    let travel = smooth((t - 0.12) / 0.76);
    let scatter = (std::f32::consts::PI * travel).sin().powi(2);
    let scale = (width as f32 * 0.025).min(height as f32 * 0.24);
    let left = width as f32 * 0.4;
    let right = width as f32 * 1.6;
    let cy = height as f32 * 2.0;
    let cx = left + (right - left) * travel;
    let yaw = 0.3 + scatter * 3.2;
    let pitch = -0.18 - scatter * 0.25;
    let color = mix([0, 198, 197], [169, 128, 238], travel);
    let project = |v: [f32; 3], center: f32| {
        let depth = 80.0 + v[2];
        Point {
            x: center + v[0] * scale * 80.0 / depth,
            y: cy + v[1] * scale * 80.0 / depth,
            depth,
        }
    };

    // Destination frames stay still; their geometry gives the movement a clear reference.
    for center in [left, right] {
        let corners = corners([0.0; 3], [7.3, 5.5, 1.8]);
        for [a, b] in EDGES {
            canvas.line(
                project(rotate(corners[a], 0.3, -0.18), center),
                project(rotate(corners[b], 0.3, -0.18), center),
                [110, 120, 130],
            );
        }
    }

    // A folder-shaped workspace: a tab and seven rows of blocks.
    for row in 0..8 {
        for column in 0..12 {
            if row == 0 && column >= 4 {
                continue;
            }
            let seed = (row * 12 + column) as u32;
            // Scatter offsets turn around the vertical axis. They shrink to zero on arrival.
            let offset = rotate(
                [
                    noise(seed * 3) * 12.0,
                    noise(seed * 3 + 1) * 14.0,
                    noise(seed * 3 + 2) * 9.0,
                ],
                seconds * 0.9,
                0.0,
            );
            let center = [
                column as f32 - 5.5 + offset[0] * scatter,
                row as f32 - 3.5 + offset[1] * scatter,
                offset[2] * scatter,
            ];
            let half = 0.46 * (1.0 - scatter * 0.65);
            let vertices = corners(center, [half, half, half]);
            let projected = vertices.map(|v| project(rotate(v, yaw, pitch), cx));
            for (indices, normal) in FACES {
                let normal = rotate(normal, yaw, pitch);
                if normal[2] >= 0.0 {
                    continue;
                }
                let light = (0.45
                    + 0.65 * (-normal[0] * 0.4 - normal[1] * 0.5 - normal[2] * 0.7).max(0.0))
                .min(1.0);
                let lit = color.map(|channel| (f32::from(channel) * light) as u8);
                let [a, b, c, d] = indices.map(|index| projected[index]);
                canvas.triangle([a, b, c], lit);
                canvas.triangle([a, c, d], lit);
            }
            // Fine particles fill the gaps as the workspace disperses.
            if scatter > 0.2 {
                let point = project(rotate(center, yaw, pitch), cx);
                canvas.dot(
                    point.x.round() as i32,
                    point.y.round() as i32,
                    point.depth - 0.5,
                    color,
                );
            }
        }
    }
}

pub fn stage(progress: f32, down: bool) -> &'static str {
    match progress {
        p if p < 0.12 => {
            if down {
                "Workspace in the sandbox"
            } else {
                "Workspace on your machine"
            }
        }
        p if p < 0.88 => {
            if down {
                "Bringing the workspace home"
            } else {
                "Moving the workspace to the sandbox"
            }
        }
        _ => {
            if down {
                "Workspace home again"
            } else {
                "Workspace in the sandbox"
            }
        }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn noise(seed: u32) -> f32 {
    let mut value = seed.wrapping_add(1).wrapping_mul(0x9e3779b9);
    value ^= value >> 16;
    value = value.wrapping_mul(0x85ebca6b);
    value ^= value >> 13;
    (value & 0xffff) as f32 / 32767.5 - 1.0
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t) as u8)
}

fn rotate([x, y, z]: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let rx = x * cy + z * sy;
    let rz = z * cy - x * sy;
    [rx, y * cp - rz * sp, y * sp + rz * cp]
}

fn corners(center: [f32; 3], half: [f32; 3]) -> [[f32; 3]; 8] {
    std::array::from_fn(|index| {
        std::array::from_fn(|axis| {
            center[axis] + half[axis] * if index & (1 << axis) == 0 { -1.0 } else { 1.0 }
        })
    })
}

const EDGES: [[usize; 2]; 12] = [
    [0, 1],
    [0, 2],
    [0, 4],
    [1, 3],
    [1, 5],
    [2, 3],
    [2, 6],
    [3, 7],
    [4, 5],
    [4, 6],
    [5, 7],
    [6, 7],
];
const FACES: [([usize; 4], [f32; 3]); 6] = [
    ([0, 2, 6, 4], [-1.0, 0.0, 0.0]),
    ([1, 5, 7, 3], [1.0, 0.0, 0.0]),
    ([0, 4, 5, 1], [0.0, -1.0, 0.0]),
    ([2, 3, 7, 6], [0.0, 1.0, 0.0]),
    ([0, 1, 3, 2], [0.0, 0.0, -1.0]),
    ([4, 6, 7, 5], [0.0, 0.0, 1.0]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_moves_between_destinations_and_frames_are_deterministic() {
        let mut canvas = Canvas::new(80, 18);
        let mut snapshots = Vec::new();
        for t in [0.0, 0.5, 1.0] {
            render(&mut canvas, 80, 18, t, 0.0);
            let cells = canvas.resolve().to_vec();
            assert!(cells.iter().filter(|c| c.glyph != ' ').count() > 50);
            render(&mut canvas, 80, 18, t, 0.0);
            assert_eq!(cells, canvas.resolve());
            snapshots.push(cells);
        }
        assert_ne!(snapshots[0], snapshots[1]);
        assert_ne!(snapshots[0], snapshots[2]);
        // The bright workspace, excluding the gray destination frames, changes sides.
        for (cells, right) in [(&snapshots[0], false), (&snapshots[2], true)] {
            let positions: Vec<_> = cells
                .iter()
                .enumerate()
                .filter(|(_, c)| c.glyph != ' ' && c.color[2] > c.color[0] + 10)
                .map(|(i, _)| i % 80)
                .collect();
            assert!(!positions.is_empty());
            let center = positions.iter().sum::<usize>() / positions.len();
            assert_eq!(center > 40, right);
        }
    }
}
