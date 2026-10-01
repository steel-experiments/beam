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

fn truecolor() -> bool {
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

/// Print one "▸ label text" progress line.
pub fn step(label: &str, text: &str) {
    say(&format!(
        "{} {} {text}",
        paint(Hue::Teal, "▸"),
        bold(Hue::Turquoise, &format!("{label:<10}"))
    ));
}

// The spinner draws on the current line. Other output clears that line first.
static SPINNER: Mutex<bool> = Mutex::new(false);

/// Print one line. When the spinner is active, clear its line first; it redraws on its next frame.
pub fn say(line: &str) {
    let active = SPINNER.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = std::io::stdout().lock();
    if *active {
        let _ = write!(out, "\r\x1b[2K");
    }
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
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
        on() && (std::env::var("TERM_PROGRAM")
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
        if self.failed {
            osc_progress(2);
            std::thread::sleep(Duration::from_millis(600));
        }
        osc_progress(0);
    }
}

// ---- Energize spinner ----

const FRAMES: [&str; 8] = ["⠁", "⠃", "⠇", "⡇", "⣇", "⣧", "⣷", "⣿"];
const SPARKS: [char; 6] = ['·', '˚', '✦', '⋆', '✧', '∗'];

fn frame(label: &str, text: &str, tick: usize, elapsed: Duration) -> String {
    let mut line = format!(
        "{} {} ",
        bold(Hue::Turquoise, FRAMES[tick % FRAMES.len()]),
        bold(Hue::Turquoise, &format!("{label:<10}"))
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
    line.push_str(&dim(&format!(" {:.0}s", elapsed.as_secs_f64())));
    line
}

/// Run slow work with the energize spinner. Without a terminal, print the plain step line instead.
pub fn task<T>(
    label: &str,
    text: &str,
    work: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if !on() {
        step(label, text);
        return work();
    }
    let short = text.trim_end_matches('…').to_string();
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
                    let mut out = std::io::stdout().lock();
                    let _ = write!(
                        out,
                        "\r\x1b[2K{}",
                        frame(&label, &short, tick, start.elapsed())
                    );
                    let _ = out.flush();
                }
                tick += 1;
                std::thread::sleep(Duration::from_millis(90));
            }
        })
    };
    let result = work();
    stop.store(true, Ordering::Relaxed);
    let _ = painter.join();
    *SPINNER.lock().unwrap_or_else(|e| e.into_inner()) = false;
    let took = dim(&format!("{:.1}s", start.elapsed().as_secs_f64()));
    let mark = if result.is_ok() {
        bold(Hue::Green, "✓")
    } else {
        bold(Hue::Red, "✗")
    };
    print!("\r\x1b[2K");
    say(&format!(
        "{mark} {} {short} {took}",
        bold(Hue::Turquoise, &format!("{label:<10}"))
    ));
    result
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
        let line = frame("upload", "abc", 3, Duration::from_secs(2));
        for c in ["a", "b", "c", "upload"] {
            assert!(line.contains(c));
        }
    }
}
