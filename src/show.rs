// ABOUTME: Transfer animation in a strip of rows above the progress line during real transfers.
// ABOUTME: It never takes over the screen: other output prints above the strip, and the strip is erased at the end.
use crate::{
    raster::{Canvas, Cell, Encoder},
    scene,
};
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Scene rows, then one row for the destination names.
const SCENE_ROWS: usize = 9;
const ROWS: usize = SCENE_ROWS + 1;
const MAX_WIDTH: usize = 72;
const FRAME: Duration = Duration::from_millis(33);
const ARRIVAL: Duration = Duration::from_millis(1200);
const GRAY: [u8; 3] = [110, 120, 130];

/// During work, progress approaches the middle of the journey: the cloud of dots between the
/// destinations. The arrival then plays the rest of the timeline.
const CAP: f32 = 0.5;
const PACE: f32 = 4.0;

struct Strip {
    width: usize,
    canvas: Canvas,
    encoder: Encoder,
    cells: Vec<Cell>,
    down: bool,
    started: Instant,
    arrival: Option<(Instant, f32)>,
}

static STRIP: Mutex<Option<Strip>> = Mutex::new(None);

/// Progress of the outward timeline after `seconds` of work.
fn working(seconds: f32) -> f32 {
    CAP * (1.0 - (-seconds / PACE).exp())
}

/// Progress during the arrival, from where the work stopped to the end.
fn arriving(from: f32, fraction: f32) -> f32 {
    let t = fraction.clamp(0.0, 1.0);
    from + (1.0 - from) * t * t * (3.0 - 2.0 * t)
}

fn strip_width(columns: u16) -> usize {
    usize::from(columns.saturating_sub(2)).min(MAX_WIDTH)
}

impl Strip {
    fn new(width: usize, down: bool) -> Self {
        Self {
            width,
            canvas: Canvas::new(width, SCENE_ROWS),
            encoder: Encoder::new(width, ROWS, crate::ui::truecolor()),
            cells: vec![Cell::default(); width * ROWS],
            down,
            started: Instant::now(),
            arrival: None,
        }
    }

    fn progress(&self) -> f32 {
        let outward = match self.arrival {
            Some((at, from)) => arriving(from, at.elapsed().as_secs_f32() / ARRIVAL.as_secs_f32()),
            None => working(self.started.elapsed().as_secs_f32()),
        };
        if self.down { 1.0 - outward } else { outward }
    }

    /// Render the scene and the destination names into `cells`.
    fn render(&mut self) {
        let width = self.width;
        let progress = self.progress();
        scene::render(
            &mut self.canvas,
            width,
            SCENE_ROWS,
            progress,
            self.started.elapsed().as_secs_f32(),
        );
        self.cells[..width * SCENE_ROWS].copy_from_slice(self.canvas.resolve());
        let names = &mut self.cells[width * SCENE_ROWS..];
        names.fill(Cell::default());
        // The scene puts the destinations at one fifth and four fifths of the width.
        for (name, center) in [("Your machine", width / 5), ("Sandbox", width * 4 / 5)] {
            let start = center.saturating_sub(name.len() / 2);
            for (cell, glyph) in names.iter_mut().skip(start).zip(name.chars()) {
                *cell = Cell { glyph, color: GRAY };
            }
        }
    }

    /// Draw the cells that changed since the last frame.
    fn draw(&mut self, out: &mut impl Write) {
        let mut output = String::new();
        self.encoder.encode_above(&self.cells, &mut output);
        if !output.is_empty() {
            let _ = write!(out, "\x1b[?2026h{output}\x1b[?2026l");
            let _ = out.flush();
        }
    }
}

/// Clear the strip rows and the cursor line, and leave the cursor at the top row of the strip.
fn erase(out: &mut impl Write) {
    let _ = write!(out, "\r\x1b[{ROWS}A\x1b[J");
}

/// True while a strip is on the screen.
pub fn active() -> bool {
    STRIP.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Before a line prints: move to the top of the strip and clear it. Returns false without a strip.
pub fn lift(out: &mut impl Write) -> bool {
    if !active() {
        return false;
    }
    erase(out);
    true
}

/// After a line printed: reserve the strip rows again below it and draw the current frame.
pub fn lower(out: &mut impl Write) {
    let mut strip = STRIP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(strip) = strip.as_mut() {
        let _ = out.write_all(&[b'\n'; ROWS]);
        strip.encoder.invalidate();
        strip.draw(out);
    }
}

/// Remove the strip after an interruption. The process exits after this.
pub fn interrupted() {
    // Release the strip lock before stdout: `say` holds stdout while it waits for the strip.
    let removed = STRIP
        .try_lock()
        .is_ok_and(|mut strip| strip.take().is_some());
    if removed {
        let mut out = std::io::stdout().lock();
        erase(&mut out);
        let _ = out.flush();
    }
}

/// The animation for one transfer. Dropping it erases the strip; `arrive` plays the arrival first.
pub struct Show {
    stop: Arc<AtomicBool>,
    painter: Option<std::thread::JoinHandle<()>>,
}

impl Show {
    /// Start the strip, or return None when motion is off or the window is too small.
    pub fn start(down: bool) -> Option<Self> {
        let (columns, rows) = crossterm::terminal::size().ok()?;
        if !crate::ui::animation()
            || std::env::var("BEAM_EFFECT").is_ok_and(|v| v == "off")
            || columns < 40
            || usize::from(rows) < ROWS + 10
            || active()
        {
            return None;
        }
        crate::transporter::cleanup_on_interrupt();
        crate::ui::exclusive(|| {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(&[b'\n'; ROWS]);
            let mut strip = Strip::new(strip_width(columns), down);
            strip.render();
            strip.draw(&mut out);
            *STRIP.lock().unwrap_or_else(|e| e.into_inner()) = Some(strip);
        });
        let stop = Arc::new(AtomicBool::new(false));
        let painter = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    paint();
                    std::thread::sleep(FRAME);
                }
            })
        };
        Some(Self {
            stop,
            painter: Some(painter),
        })
    }

    /// Play the arrival, then erase the strip.
    pub fn arrive(self) {
        if let Some(strip) = STRIP.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let from = working(strip.started.elapsed().as_secs_f32());
            strip.arrival = Some((Instant::now(), from));
        }
        std::thread::sleep(ARRIVAL + FRAME * 2);
    }
}

/// One frame. A changed window width clears the strip and draws it again at the new width.
fn paint() {
    crate::ui::exclusive(|| {
        let mut guard = STRIP.lock().unwrap_or_else(|e| e.into_inner());
        let Some(strip) = guard.as_mut() else {
            return;
        };
        let mut out = std::io::stdout().lock();
        if let Ok((columns, _)) = crossterm::terminal::size()
            && strip_width(columns) != strip.width
            && columns >= 40
        {
            erase(&mut out);
            let _ = write!(out, "\x1b[{ROWS}B");
            let mut resized = Strip::new(strip_width(columns), strip.down);
            resized.started = strip.started;
            resized.arrival = strip.arrival;
            *strip = resized;
        }
        strip.render();
        strip.draw(&mut out);
    });
}

impl Drop for Show {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(painter) = self.painter.take() {
            let _ = painter.join();
        }
        crate::ui::exclusive(|| {
            if STRIP
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .is_some()
            {
                let mut out = std::io::stdout().lock();
                erase(&mut out);
                let _ = out.flush();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_holds_the_cloud_between_destinations_and_arrival_completes_the_journey() {
        assert_eq!(working(0.0), 0.0);
        let mut previous = 0.0;
        for seconds in [0.5, 2.0, 8.0, 60.0, 3600.0] {
            let progress = working(seconds);
            assert!(
                progress > previous && progress <= CAP,
                "{seconds}: {progress}"
            );
            previous = progress;
        }
        assert!(working(3600.0) > CAP - 0.001);
        assert_eq!(arriving(0.3, 0.0), 0.3);
        assert!(arriving(0.3, 0.5) > 0.3 && arriving(0.3, 0.5) < 1.0);
        assert_eq!(arriving(0.3, 1.0), 1.0);
        assert_eq!(arriving(0.3, 7.0), 1.0);
    }

    #[test]
    fn strip_shows_both_destination_names_and_the_workspace() {
        let mut strip = Strip::new(60, false);
        strip.render();
        let names: String = strip.cells[60 * SCENE_ROWS..]
            .iter()
            .map(|c| c.glyph)
            .collect();
        assert!(
            names.contains("Your machine") && names.contains("Sandbox"),
            "{names:?}"
        );
        assert!(
            strip.cells[..60 * SCENE_ROWS]
                .iter()
                .any(|c| c.glyph != ' ')
        );
        assert_eq!(strip_width(300), MAX_WIDTH);
        assert_eq!(strip_width(50), 48);
    }
}
