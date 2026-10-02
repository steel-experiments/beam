// ABOUTME: Preview Beam's progress effects without allocating a sandbox or transferring files.
// ABOUTME: Use BEAM_EFFECT=off|graphics|shader and pass down to reverse the direction.
#![allow(dead_code)]
#[path = "../src/transporter.rs"]
mod transporter;
#[path = "../src/ui.rs"]
mod ui;

fn main() -> anyhow::Result<()> {
    let home = std::env::args().any(|arg| arg == "down");
    let message = std::env::args().any(|arg| arg == "message");
    transporter::direction(home);
    let _busy = ui::Busy::start();
    ui::say("Beam display preview — no files are transferred.");
    ui::step("destination", if home { "home" } else { "preview sandbox" });
    ui::task("check", "checking preview", || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        Ok(())
    })?;
    ui::task(
        if home { "download" } else { "upload" },
        "previewing transfer effect",
        || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if message {
                ui::say("A mid-transfer message remains visible.");
            }
            std::thread::sleep(std::time::Duration::from_secs(3));
            Ok(())
        },
    )?;
    ui::success("Preview complete. Previous output stays in scrollback.");
    Ok(())
}
