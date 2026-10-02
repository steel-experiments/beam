// ABOUTME: Optional graphics beneath the current progress line; never owns the screen.
// ABOUTME: Ghostty shader signaling is opt-in, and unknown terminals keep the inline spinner.
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static HOME: AtomicBool = AtomicBool::new(false);
static ACTIVE: Mutex<Option<Effect>> = Mutex::new(None);
static HANDLER: OnceLock<bool> = OnceLock::new();
static CLOCK: OnceLock<Instant> = OnceLock::new();
static NEXT_IMAGE: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Inline,
    Graphics,
    Shader,
}

// Do not probe stdin or send unsupported escape sequences through multiplexers.
fn mode(request: &str, terminal: &str, kitty: bool, multiplexed: bool, enabled: bool) -> Mode {
    if !enabled || multiplexed {
        return Mode::Inline;
    }
    match request {
        "shader" if terminal == "ghostty" => Mode::Shader,
        "auto" | "graphics" if terminal == "ghostty" || kitty => Mode::Graphics,
        _ => Mode::Inline,
    }
}

pub fn direction(home: bool) {
    HOME.store(home, Ordering::Relaxed);
}

struct Effect {
    mode: Mode,
    id: u32,
    home: bool,
    started: Instant,
    armed: bool,
}

/// An effect can end early when work prints a message, and is also cleaned up on unwind.
pub struct Ambient;
impl Ambient {
    pub fn start() -> Option<Self> {
        let mode = mode(
            &std::env::var("BEAM_EFFECT").unwrap_or_else(|_| "auto".into()),
            &std::env::var("TERM_PROGRAM").unwrap_or_default(),
            std::env::var_os("KITTY_WINDOW_ID").is_some(),
            std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some(),
            crate::ui::animation(),
        );
        if mode == Mode::Inline {
            return None;
        }
        if !cleanup_on_interrupt() {
            return None;
        }
        let id = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
        // Image ids are private to this process/run, never delete all terminal images.
        let id = (std::process::id().wrapping_mul(65537).wrapping_add(id)) | 0x80000000;
        *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Effect {
            mode,
            id,
            home: HOME.load(Ordering::Relaxed),
            started: *CLOCK.get_or_init(Instant::now),
            armed: false,
        });
        Some(Self)
    }
}
impl Drop for Ambient {
    fn drop(&mut self) {
        finish();
    }
}

/// Install one interrupt handler that removes progress effects and exits with status 130.
/// Returns false when another handler is already installed.
pub fn cleanup_on_interrupt() -> bool {
    *HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| {
            finish();
            crate::show::interrupted();
            std::process::exit(130);
        })
        .is_ok()
    })
}

fn cleanup(effect: &Effect) -> String {
    if !effect.armed {
        return String::new();
    }
    match effect.mode {
        Mode::Graphics => format!("\x1b_Ga=d,d=I,i={},q=2\x1b\\", effect.id),
        // Shader opt-in temporarily owns cursor color; return it to the configured theme.
        Mode::Shader => "\x1b]112\x1b\\".into(),
        Mode::Inline => String::new(),
    }
}

pub fn finish() {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(effect) = active.take() {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(cleanup(&effect).as_bytes());
        let _ = out.flush();
    }
}

/// Paint only the current line, with image content beneath it and no cursor displacement.
/// Lock ordering is always effect -> stdout, including ordinary output and cancellation.
pub fn draw(line: &str, elapsed: Duration) {
    let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let Some(effect) = active.as_mut() else {
        let _ = write!(out, "\r\x1b[2K{line}");
        let _ = out.flush();
        return;
    };
    let (width, _) = crossterm::terminal::size().unwrap_or((0, 0));
    // Never introduce the effect for quick steps. Keep both overlay and status on one row.
    let columns = visible_width(line);
    if width < 40 || columns >= usize::from(width.saturating_sub(1)) {
        let _ = out.write_all(cleanup(effect).as_bytes());
        *active = None;
        let _ = write!(out, "\r\x1b[2K{line}");
    } else if elapsed < Duration::from_millis(600) {
        let _ = write!(out, "\r\x1b[2K{line}");
    } else {
        let mut frame = String::from("\x1b[?2026h\r\x1b[2K");
        effect.armed = true;
        match effect.mode {
            Mode::Graphics => {
                if let Ok(image) = image_command(
                    effect.id,
                    (width - 1).min(72),
                    effect.started.elapsed().as_secs_f64(),
                    effect.home,
                ) {
                    frame.push_str(&image);
                }
            }
            Mode::Shader => frame.push_str(if effect.home {
                "\x1b]12;#a980ee\x1b\\"
            } else {
                "\x1b]12;#0bcae1\x1b\\"
            }),
            Mode::Inline => {}
        }
        frame.push_str(line);
        frame.push_str("\x1b[?2026l");
        let _ = out.write_all(frame.as_bytes());
    }
    let _ = out.flush();
}

pub(crate) fn visible_width(line: &str) -> usize {
    let mut escape = false;
    line.chars()
        .filter(|&c| {
            if c == '\x1b' {
                escape = true;
            }
            if escape {
                if c == 'm' {
                    escape = false;
                }
                false
            } else {
                true
            }
        })
        .count()
}

const PIXELS_W: usize = 288;
const PIXELS_H: usize = 24;

// Smooth translucent shafts, not textual particles. Alpha stays low enough for light themes.
fn pixels(seconds: f64, home: bool) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(PIXELS_W * PIXELS_H * 4);
    let time = if home { seconds } else { -seconds };
    for y in 0..PIXELS_H {
        let v = y as f64 / (PIXELS_H - 1) as f64;
        let edge = (std::f64::consts::PI * v).sin().powi(2);
        for x in 0..PIXELS_W {
            let u = x as f64 / PIXELS_W as f64;
            let shaft = (u * 15.0 + (u * 9.0).sin() * 0.3).sin().abs().powi(12);
            let wave = 0.5 + 0.5 * (v * 7.0 + time * 1.1 + u * 4.0).sin();
            let envelope = (std::f64::consts::PI * u).sin().powi(2);
            let alpha = (edge * envelope * (5.0 + shaft * wave * 30.0)) as u8;
            rgba.extend_from_slice(&[0, 198, 197, alpha]);
        }
    }
    rgba
}

fn image_command(id: u32, columns: u16, seconds: f64, home: bool) -> std::io::Result<String> {
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::fast());
    compressor.write_all(&pixels(seconds, home))?;
    let payload = STANDARD.encode(compressor.finish()?);
    let mut command = String::new();
    // APC payload chunks must be <=4096 bytes and preserve four-byte base64 boundaries.
    for (i, chunk) in payload.as_bytes().chunks(4096).enumerate() {
        use std::fmt::Write as _;
        let more = u8::from((i + 1) * 4096 < payload.len());
        if i == 0 {
            let _ = write!(
                command,
                "\x1b_Ga=T,f=32,o=z,s={PIXELS_W},v={PIXELS_H},i={id},p=1,c={columns},r=1,C=1,z=-1,q=2,m={more};"
            );
        } else {
            let _ = write!(command, "\x1b_Gm={more},q=2;");
        }
        command.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        command.push_str("\x1b\\");
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effects_require_a_known_terminal_and_respect_opt_out_and_multiplexers() {
        assert_eq!(mode("auto", "ghostty", false, false, true), Mode::Graphics);
        assert_eq!(mode("auto", "", true, false, true), Mode::Graphics);
        assert_eq!(mode("shader", "ghostty", false, false, true), Mode::Shader);
        for (request, terminal, mux, enabled) in [
            ("auto", "xterm", false, true),
            ("shader", "kitty", false, true),
            ("auto", "ghostty", true, true),
            ("auto", "ghostty", false, false),
            ("off", "ghostty", false, true),
        ] {
            assert_eq!(mode(request, terminal, false, mux, enabled), Mode::Inline);
        }
    }
    #[test]
    fn overlay_is_transparent_bounded_and_directional() {
        let up = pixels(2.0, false);
        let down = pixels(2.0, true);
        assert_eq!(up.len(), PIXELS_W * PIXELS_H * 4);
        assert_ne!(up, down);
        for pixel in up.as_chunks::<4>().0 {
            assert!(pixel[3] <= 35);
        }
        assert!(
            up[..PIXELS_W * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 0)
        );
    }
    #[test]
    fn graphics_frames_do_not_clear_the_screen_or_move_the_cursor() {
        let command = image_command(123, 60, 2.0, false).unwrap();
        assert!(command.contains("C=1,z=-1,q=2"));
        assert!(!command.contains("1049") && !command.contains("[2J"));
        let mut encoded = String::new();
        for chunk in command.split("\x1b_G").skip(1) {
            let payload = chunk.split_once(';').unwrap().1.trim_end_matches("\x1b\\");
            assert!(payload.len() <= 4096);
            encoded.push_str(payload);
        }
        let compressed = STANDARD.decode(encoded).unwrap();
        use std::io::Read;
        let mut decoded = Vec::new();
        flate2::read::ZlibDecoder::new(&compressed[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, pixels(2.0, false));
        let effect = Effect {
            mode: Mode::Graphics,
            id: 123,
            home: false,
            started: Instant::now(),
            armed: true,
        };
        assert_eq!(cleanup(&effect), "\x1b_Ga=d,d=I,i=123,q=2\x1b\\");
    }
}
