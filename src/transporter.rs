// ABOUTME: A sparse, cell-based transporter field during slow terminal tasks.
// ABOUTME: Alternate-screen ownership is temporary; summaries stay in shell scrollback.
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static HOME: AtomicBool = AtomicBool::new(false);
static HANDLER: OnceLock<bool> = OnceLock::new();

pub fn direction(home: bool) {
    HOME.store(home, Ordering::Relaxed);
}

pub struct Screen;
impl Screen {
    pub fn start(text: &str) -> Option<Self> {
        if !crate::ui::on()
            || std::env::var("BEAM_ANIMATION").is_ok_and(|v| v == "0")
            || !crossterm::terminal::size()
                .is_ok_and(|(w, h)| usize::from(w) >= text.chars().count() + 32 && h >= 10)
        {
            return None;
        }
        // Do not take screen ownership unless cancellation can restore it.
        if !*HANDLER.get_or_init(|| {
            ctrlc::set_handler(|| {
                finish();
                std::process::exit(130);
            })
            .is_ok()
        }) {
            return None;
        }
        let mut out = std::io::stdout().lock();
        ACTIVE.store(true, Ordering::SeqCst);
        if write!(out, "\x1b[?1049h\x1b[?25l")
            .and_then(|_| out.flush())
            .is_err()
        {
            drop(out);
            finish();
            return None;
        }
        Some(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        finish();
    }
}

pub fn finish() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    }
}

// Deterministic particles drift vertically and fade over their lifetime.
fn particle(i: u32, seconds: f64, width: u16, height: u16, home: bool) -> (u16, u16, u8) {
    let phase = (seconds * (0.07 + (i % 5) as f64 * 0.013) + i as f64 * 0.618).fract();
    let x = 1 + ((i.wrapping_mul(2654435761) >> 8) % u32::from(width - 2)) as u16;
    let y = 1 + ((if home { phase } else { 1.0 - phase }) * f64::from(height - 2)) as u16;
    let brightness = (12.0 + 65.0 * (std::f64::consts::PI * phase).sin().powi(2)) as u8;
    (x, y, brightness)
}

pub fn draw(out: &mut impl Write, line: &str, elapsed: Duration) {
    if !ACTIVE.load(Ordering::SeqCst) {
        let _ = write!(out, "\r\x1b[2K{line}");
        return;
    }
    let Ok((width, height)) = crossterm::terminal::size() else {
        return;
    };
    if width < 4 || height < 4 {
        return;
    }
    let row = height / 2;
    // Build the whole frame before writing, keeping terminal updates together.
    let mut frame = String::from("\x1b[?2026h\x1b[H\x1b[2J");
    use std::fmt::Write as _;
    let line = clipped(line, width.saturating_sub(4) as usize);
    let home = HOME.load(Ordering::Relaxed);
    let count = (u32::from(width) * u32::from(height) / 65).clamp(8, 100);
    for i in 0..count {
        let (x, y, b) = particle(i, elapsed.as_secs_f64(), width, height, home);
        if y.abs_diff(row) <= 2 {
            continue;
        }
        let glyph = if i % 9 == 0 { ':' } else { '.' };
        let _ = write!(frame, "\x1b[{y};{x}H\x1b[38;2;{};{b};{b}m{glyph}", b / 4);
    }
    let title = if home {
        "REMATERIALIZING"
    } else {
        "ENERGIZING"
    };
    let _ = write!(
        frame,
        "\x1b[{};3H\x1b[0m\x1b[2m{title}\x1b[0m\x1b[{row};3H{line}\x1b[0m",
        row.saturating_sub(2)
    );
    frame.push_str("\x1b[?2026l");
    let _ = out.write_all(frame.as_bytes());
}

// Preserve styling while limiting the single-cell glyphs used by the progress line.
fn clipped(line: &str, columns: usize) -> String {
    let mut result = String::new();
    let mut escape = false;
    let mut used = 0;
    for c in line.chars() {
        if c == '\x1b' {
            escape = true;
        }
        if escape {
            result.push(c);
            if c == 'm' {
                escape = false;
            }
        } else {
            if used == columns {
                break;
            }
            result.push(c);
            used += 1;
        }
    }
    result.push_str("\x1b[0m");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resized_status_preserves_styles_and_fits() {
        assert_eq!(clipped("\x1b[31mhello\x1b[0m", 3), "\x1b[31mhel\x1b[0m");
    }
    #[test]
    fn particles_stay_in_bounds_and_reverse_direction() {
        for i in 0..100 {
            for tick in 0..200 {
                let (x, up, b) = particle(i, tick as f64 / 10.0, 80, 24, false);
                let (_, down, _) = particle(i, tick as f64 / 10.0, 80, 24, true);
                assert!((1..80).contains(&x));
                assert!((1..24).contains(&up));
                assert!((1..24).contains(&down));
                assert!((12..=77).contains(&b));
                assert!(up.abs_diff(24 - down) <= 1);
            }
        }
    }
}
