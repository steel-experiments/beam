// ABOUTME: Terminal decoration for people: palette colors, links, tab progress, the energize spinner, and flavor text.
// ABOUTME: Decoration is off when stdout is not a terminal or NO_COLOR is set, so scripts and tests get plain text.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Hue {
    White,
    Turquoise,
    Purple,
    Green,
    Teal,
    BabyBlue,
    Blue,
    Yellow,
    PaleYellow,
    Orange,
    Red,
}

impl Hue {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Hue::White => (0xfd, 0xfd, 0xfc),
            Hue::Turquoise => (0x00, 0xc6, 0xc5),
            Hue::Purple => (0x79, 0x33, 0x87),
            Hue::Green => (0x80, 0xaa, 0x40),
            Hue::Teal => (0x00, 0x80, 0x80),
            Hue::BabyBlue => (0xd7, 0xf0, 0xff),
            Hue::Blue => (0x05, 0xa5, 0xff),
            Hue::Yellow => (0xf4, 0xdd, 0x15),
            Hue::PaleYellow => (0xfa, 0xea, 0x72),
            Hue::Orange => (0xdf, 0x81, 0x20),
            Hue::Red => (0xd4, 0x00, 0x00),
        }
    }

    /// The nearest xterm 256-color index, for terminals without 24-bit color.
    fn xterm(self) -> u8 {
        match self {
            Hue::White => 231,
            Hue::Turquoise => 44,
            Hue::Purple => 96,
            Hue::Green => 107,
            Hue::Teal => 30,
            Hue::BabyBlue => 195,
            Hue::Blue => 39,
            Hue::Yellow => 220,
            Hue::PaleYellow => 221,
            Hue::Orange => 172,
            Hue::Red => 160,
        }
    }

    pub fn color(self) -> anstyle::Color {
        if truecolor() {
            let (r, g, b) = self.rgb();
            anstyle::RgbColor(r, g, b).into()
        } else {
            anstyle::Ansi256Color(self.xterm()).into()
        }
    }
}

pub fn truecolor() -> bool {
    static TRUECOLOR: OnceLock<bool> = OnceLock::new();
    *TRUECOLOR.get_or_init(|| {
        std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
            || std::env::var("TERM_PROGRAM")
                .is_ok_and(|v| matches!(v.as_str(), "iTerm.app" | "WezTerm" | "ghostty" | "vscode"))
    })
}

fn allowed(is_terminal: bool) -> bool {
    is_terminal
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

/// True when stdout is a terminal that accepts decoration.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| allowed(std::io::stdout().is_terminal()))
}

/// True when stderr is a terminal that accepts decoration.
pub fn err_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| allowed(std::io::stderr().is_terminal()))
}

/// One switch for all animated output, including the terminal's busy indicator.
pub fn animation() -> bool {
    motion_allowed(on(), std::env::var("BEAM_ANIMATION").ok().as_deref())
}

fn motion_allowed(decorated: bool, setting: Option<&str>) -> bool {
    decorated && setting != Some("0")
}

fn styled(enabled: bool, style: anstyle::Style, text: &str) -> String {
    if enabled {
        format!("{style}{text}{style:#}")
    } else {
        text.to_string()
    }
}

pub fn paint(hue: Hue, text: &str) -> String {
    styled(
        on(),
        anstyle::Style::new().fg_color(Some(hue.color())),
        text,
    )
}

pub fn bold(hue: Hue, text: &str) -> String {
    styled(
        on(),
        anstyle::Style::new().fg_color(Some(hue.color())).bold(),
        text,
    )
}

pub fn dim(text: &str) -> String {
    styled(on(), anstyle::Style::new().dimmed(), text)
}

/// The error line for stderr: a red cross, then the message.
pub fn error(message: &str) -> String {
    let style = anstyle::Style::new()
        .fg_color(Some(Hue::Red.color()))
        .bold();
    format!("{} {message}", styled(err_on(), style, "✗"))
}

/// An error whose message is already on stderr. main exits with failure and does not print it again.
#[derive(Debug)]
pub struct Reported;

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error already reported")
    }
}

impl std::error::Error for Reported {}

/// Print "✓ message" with a green check.
pub fn success(message: impl AsRef<str>) {
    say(&format!("{} {}", bold(Hue::Green, "✓"), message.as_ref()));
}

/// Print "! message" with an orange mark.
pub fn warn(message: impl AsRef<str>) {
    say(&format!(
        "{} {}",
        bold(Hue::Orange, "!"),
        paint(Hue::Orange, message.as_ref())
    ));
}

/// Print "Next: command" with the command in bold so that it is easy to find and copy.
pub fn next(command: impl AsRef<str>) {
    say(&format!(
        "{} {}",
        dim("Next:"),
        bold(Hue::Blue, command.as_ref())
    ));
}

/// Text in bold with the terminal's own foreground color, which works on dark and light themes.
pub fn strong(text: &str) -> String {
    styled(on(), anstyle::Style::new().bold(), text)
}

/// Print "◆ text" as the heading of a choice or summary.
pub fn heading(text: impl AsRef<str>) {
    say(&format!(
        "{} {}",
        bold(Hue::Purple, "◆"),
        strong(text.as_ref())
    ));
}

/// A question line: a "?" mark, the question in bold, and the bracketed choices dimmed.
pub fn question(prompt: &str) -> String {
    if !on() {
        return prompt.to_string();
    }
    let (text, hint) = prompt.split_at(prompt.find('[').unwrap_or(prompt.len()));
    format!(
        "{} {}{}",
        bold(Hue::Turquoise, "?"),
        strong(text),
        dim(hint)
    )
}

/// "Label: value" with the label dimmed so that the value stands out.
pub fn field(label: &str, value: impl std::fmt::Display) -> String {
    format!("{} {value}", dim(&format!("{label}:")))
}

/// A "Beam: message" notice for stderr, with the prefix in turquoise.
pub fn notice(message: &str) -> String {
    let style = anstyle::Style::new()
        .fg_color(Some(Hue::Turquoise.color()))
        .bold();
    format!("{} {message}", styled(err_on(), style, "Beam:"))
}

/// Step labels share one column. The longest label is "destination".
const LABEL: usize = 11;

/// Print one "▸ label text" progress line.
pub fn step(label: &str, text: &str) {
    say(&format!(
        "{} {} {text}",
        paint(Hue::Teal, "▸"),
        bold(Hue::Turquoise, &format!("{label:<LABEL$}"))
    ));
}

// The spinner draws on the current line. Other output clears that line first.
static SPINNER: Mutex<bool> = Mutex::new(false);

/// Print one line. When the spinner is active, clear its line first; it redraws on its next frame.
/// A transfer animation moves below the line.
pub fn say(line: &str) {
    crate::transporter::finish();
    let active = SPINNER.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    let lifted = crate::show::lift(&mut out);
    if *active && !lifted {
        let _ = write!(out, "\r\x1b[2K");
    }
    let _ = writeln!(out, "{line}");
    if lifted {
        crate::show::lower(&mut out);
    }
    let _ = out.flush();
}

/// Run terminal drawing that must not interleave with progress lines or `say`.
pub fn exclusive<R>(draw: impl FnOnce() -> R) -> R {
    let _guard = SPINNER.lock().unwrap_or_else(|e| e.into_inner());
    draw()
}

// ---- Hyperlinks (OSC 8) ----

fn hyperlinks() -> bool {
    static LINKS: OnceLock<bool> = OnceLock::new();
    *LINKS.get_or_init(|| {
        if std::env::var("FORCE_HYPERLINK").is_ok_and(|v| v != "0") {
            return true;
        }
        on() && (std::env::var("TERM_PROGRAM").is_ok_and(|v| {
            matches!(
                v.as_str(),
                "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "Hyper"
            )
        }) || std::env::var_os("WT_SESSION").is_some()
            || std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var("VTE_VERSION")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|v| v >= 5000))
    })
}

fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// The path as text. Terminals that support OSC 8 also make it a clickable link.
pub fn link(path: &Path) -> String {
    let text = path.display().to_string();
    if hyperlinks() && path.is_absolute() {
        format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            file_url(path),
            paint(Hue::BabyBlue, &text)
        )
    } else {
        text
    }
}

// ---- Tab and taskbar progress (OSC 9;4) ----

fn tab_progress() -> bool {
    static PROGRESS: OnceLock<bool> = OnceLock::new();
    *PROGRESS.get_or_init(|| {
        animation()
            && (std::env::var("TERM_PROGRAM")
                .is_ok_and(|v| matches!(v.as_str(), "ghostty" | "WezTerm" | "iTerm.app"))
                || std::env::var_os("WT_SESSION").is_some()
                || std::env::var_os("ConEmuPID").is_some())
    })
}

fn osc_progress(state: u8) {
    if tab_progress() {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b]9;4;{state};\x1b\\");
        let _ = out.flush();
    }
}

/// Shows a busy indicator in the terminal tab until it is dropped.
pub struct Busy {
    failed: bool,
}

impl Busy {
    pub fn start() -> Busy {
        osc_progress(3);
        Busy { failed: false }
    }

    /// Mark the work as failed. The tab shows the error state briefly when supported.
    pub fn fail(&mut self) {
        self.failed = true;
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.failed && animation() {
            osc_progress(2);
            std::thread::sleep(Duration::from_millis(600));
        }
        osc_progress(0);
    }
}

// ---- Energize spinner ----

const FRAMES: [&str; 8] = ["⠁", "⠃", "⠇", "⡇", "⣇", "⣧", "⣷", "⣿"];
const SPARKS: [char; 6] = ['·', '˚', '✦', '⋆', '✧', '∗'];

/// One spinner line. It is shorter than `width`, so it never wraps.
fn frame(label: &str, text: &str, tick: usize, elapsed: Duration, width: usize) -> String {
    let seconds = format!("{:.0}s", elapsed.as_secs_f64());
    let available = width.saturating_sub(1);
    // Spinner, label, text, four sparks, and seconds, with a space between each part.
    let fixed = label.len().max(LABEL) + seconds.len() + 9;
    if available <= fixed {
        let compact = format!("{} {seconds}", FRAMES[tick % FRAMES.len()]);
        return compact.chars().take(available).collect();
    }
    let budget = available - fixed;
    let text: String = if text.len() > budget {
        format!("{}…", &text[..budget - 1])
    } else {
        text.into()
    };
    let mut line = format!(
        "{} {} ",
        bold(Hue::Turquoise, FRAMES[tick % FRAMES.len()]),
        bold(Hue::Turquoise, &format!("{label:<LABEL$}"))
    );
    // A bright band moves across the text, like a pattern that resolves.
    let chars: Vec<char> = text.chars().collect();
    let band = tick % (chars.len() + 8);
    for (i, c) in chars.iter().enumerate() {
        let hue = match band.abs_diff(i) {
            0 => Hue::White,
            1 => Hue::BabyBlue,
            2 => Hue::Turquoise,
            _ => Hue::Teal,
        };
        line.push_str(&paint(hue, &c.to_string()));
    }
    line.push(' ');
    for i in 0..4 {
        let spark = SPARKS[(tick * 7 + i * 3) % SPARKS.len()];
        let hue = [Hue::PaleYellow, Hue::Turquoise, Hue::BabyBlue, Hue::Blue][(tick + i) % 4];
        line.push_str(&paint(hue, &spark.to_string()));
    }
    line.push_str(&dim(&format!(" {seconds}")));
    line
}

/// Run slow work with the energize spinner. Without a terminal, print the plain step line instead.
pub fn task<T>(
    label: &str,
    text: &str,
    work: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    // Static output wraps normally. Animated output uses ASCII text so its cell width is unambiguous.
    let short = text.trim_end_matches('…').to_string();
    if !animation() || !short.is_ascii() || !label.is_ascii() {
        step(label, text);
        let started = Instant::now();
        let result = work();
        task_result(
            label,
            text.trim_end_matches('…'),
            result.is_ok(),
            started.elapsed(),
        );
        return result;
    }
    let ambient = crate::transporter::Ambient::start();
    let stop = Arc::new(AtomicBool::new(false));
    *SPINNER.lock().unwrap_or_else(|e| e.into_inner()) = true;
    let start = Instant::now();
    let painter = {
        let stop = stop.clone();
        let label = label.to_string();
        let short = short.clone();
        std::thread::spawn(move || {
            let mut tick = 0;
            while !stop.load(Ordering::Relaxed) {
                {
                    let _guard = SPINNER.lock().unwrap_or_else(|e| e.into_inner());
                    let width = crossterm::terminal::size().map_or(80, |size| usize::from(size.0));
                    let line = frame(&label, &short, tick, start.elapsed(), width);
                    crate::transporter::draw(&line, start.elapsed());
                }
                tick += 1;
                std::thread::sleep(Duration::from_millis(90));
            }
        })
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
    stop.store(true, Ordering::Relaxed);
    let _ = painter.join();
    *SPINNER.lock().unwrap_or_else(|e| e.into_inner()) = false;
    drop(ambient);
    let result = match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    print!("\r\x1b[2K");
    task_result(label, &short, result.is_ok(), start.elapsed());
    result
}

fn task_result(label: &str, text: &str, success: bool, elapsed: Duration) {
    let mark = if success {
        bold(Hue::Green, "✓")
    } else {
        bold(Hue::Red, "✗")
    };
    say(&format!(
        "{mark} {} {text} {}",
        bold(Hue::Turquoise, &format!("{label:<LABEL$}")),
        dim(&format!("{:.1}s", elapsed.as_secs_f64()))
    ));
}

// ---- Arrival signals and flavor ----

/// After slow work, ring the bell and send a desktop notification on terminals that support OSC 9.
pub fn arrived(started: Instant, message: &str) {
    if !on() || started.elapsed() < Duration::from_secs(15) {
        return;
    }
    let notify = std::env::var("TERM_PROGRAM")
        .is_ok_and(|v| matches!(v.as_str(), "iTerm.app" | "ghostty" | "WezTerm"));
    let mut out = std::io::stdout().lock();
    if notify {
        let clean: String = message.chars().filter(|c| !c.is_control()).collect();
        let _ = write!(out, "\x1b]9;{clean}\x1b\\");
    }
    let _ = write!(out, "\x07");
    let _ = out.flush();
}

const UP_LINES: [&str; 5] = [
    "Energize.",
    "No redshirts were lost.",
    "Transporter buffer nominal.",
    "All molecules accounted for.",
    "Pattern lock held the whole way.",
];
const DOWN_LINES: [&str; 5] = [
    "Welcome home.",
    "Pattern buffer clean. Nothing left behind.",
    "Rematerialized with the same number of fingers.",
    "The away team is back aboard.",
    "Scotty would be proud.",
];

/// A random number from the standard library's per-process hash seed. Clock nanoseconds are not random:
/// on macOS they are always a multiple of 1000.
fn roll() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// One line in 20 rolls, else None.
fn rare_pick(lines: &[&'static str], roll: u64) -> Option<&'static str> {
    roll.is_multiple_of(20)
        .then(|| lines[(roll / 20) as usize % lines.len()])
}

/// Sometimes, after a successful move, add a short line of flavor text.
pub fn flavor(home: bool) {
    if on()
        && let Some(line) = rare_pick(if home { &DOWN_LINES } else { &UP_LINES }, roll())
    {
        say(&paint(Hue::Purple, line));
    }
}

/// The reply to `beam me up scotty`.
pub fn scotty() {
    say(&paint(
        Hue::Purple,
        "Scotty is on a break. Beam will operate the transporter.",
    ));
}

/// Rewrite `beam me up|down [scotty] ...` to `beam up|down ...`. Return the arguments and whether Scotty was asked.
pub fn me_alias(mut args: Vec<std::ffi::OsString>) -> (Vec<std::ffi::OsString>, bool) {
    fn word(args: &[std::ffi::OsString], i: usize) -> String {
        args.get(i)
            .and_then(|a| a.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    }
    if word(&args, 1) != "me" || !matches!(word(&args, 2).as_str(), "up" | "down") {
        return (args, false);
    }
    args.remove(1);
    let scotty = word(&args, 2).trim_end_matches([',', '!', '.']) == "scotty";
    if scotty {
        args.remove(2);
    }
    (args, scotty)
}

// ---- Help colors ----

pub fn help_styles() -> clap::builder::Styles {
    use anstyle::Style;
    let raw = |hue: Hue| {
        let (r, g, b) = hue.rgb();
        anstyle::Color::from(anstyle::RgbColor(r, g, b))
    };
    clap::builder::Styles::styled()
        .header(Style::new().fg_color(Some(raw(Hue::Turquoise))).bold())
        .usage(Style::new().fg_color(Some(raw(Hue::Turquoise))).bold())
        .literal(Style::new().fg_color(Some(raw(Hue::Blue))).bold())
        .placeholder(Style::new().fg_color(Some(raw(Hue::BabyBlue))))
        .valid(Style::new().fg_color(Some(raw(Hue::Green))))
        .invalid(Style::new().fg_color(Some(raw(Hue::Orange))).bold())
        .error(Style::new().fg_color(Some(raw(Hue::Red))).bold())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn me_alias_rewrites_only_up_and_down() {
        assert_eq!(
            me_alias(args(&["beam", "me", "up", "--to", "steel"])),
            (args(&["beam", "up", "--to", "steel"]), false)
        );
        assert_eq!(
            me_alias(args(&["beam", "me", "up", "Scotty!", "-y"])),
            (args(&["beam", "up", "-y"]), true)
        );
        assert_eq!(
            me_alias(args(&["beam", "me", "down"])),
            (args(&["beam", "down"]), false)
        );
        assert_eq!(
            me_alias(args(&["beam", "me"])),
            (args(&["beam", "me"]), false)
        );
        assert_eq!(
            me_alias(args(&["beam", "up", "me"])),
            (args(&["beam", "up", "me"]), false)
        );
    }

    #[test]
    fn flavor_text_is_rare() {
        let hits = (0..2000)
            .filter(|_| rare_pick(&UP_LINES, roll()).is_some())
            .count();
        assert!(
            (40..=200).contains(&hits),
            "{hits} of 2000 rolls picked a line"
        );
        assert_eq!(rare_pick(&UP_LINES, 0), Some(UP_LINES[0]));
        assert_eq!(rare_pick(&UP_LINES, 21), None);
    }

    #[test]
    fn file_urls_escape_unsafe_bytes() {
        assert_eq!(
            file_url(Path::new("/tmp/my project/ü")),
            "file:///tmp/my%20project/%C3%BC"
        );
    }

    #[test]
    fn spinner_frame_contains_the_text_characters() {
        let line = frame("upload", "abc", 3, Duration::from_secs(2), 80);
        for c in ["a", "b", "c", "upload"] {
            assert!(line.contains(c));
        }
    }

    #[test]
    fn progress_does_not_wrap_when_the_window_shrinks() {
        for width in [0, 1, 2, 12, 24, 40, 80] {
            let line = frame(
                "destination",
                "a long operation description that needs clipping",
                3,
                Duration::from_secs(123),
                width,
            );
            assert!(crate::transporter::visible_width(&line) < width.max(1));
        }
    }

    #[test]
    fn motion_requires_decoration_and_respects_the_global_switch() {
        assert!(motion_allowed(true, None));
        assert!(motion_allowed(true, Some("1")));
        assert!(!motion_allowed(true, Some("0")));
        assert!(!motion_allowed(false, Some("1")));
    }
}
