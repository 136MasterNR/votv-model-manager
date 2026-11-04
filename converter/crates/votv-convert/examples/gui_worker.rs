//! How a GUI should drive the converter: inspect on file pick, convert on a worker thread,
//! progress/log over a channel, cancel from the UI thread.
//!
//!     cargo run --release --example gui_worker -- path/to/model.glb

use std::sync::mpsc;
use std::thread;
use votv_convert::*;

enum Msg {
    Progress(Progress),
    Log(Diagnostic),
    Finished(Result<Report>),
}

fn main() {
    let input = std::env::args()
        .nth(1)
        .expect("usage: gui_worker <model.glb>");
    let input = std::path::PathBuf::from(input);

    // 1. The user picked a file: look inside it (fast) and pre-fill the options form.
    let info = inspect(&input).expect("inspect");
    println!(
        "{} parts, {} triangles, {:.0} x {:.0} x {:.0} cm",
        info.visible_parts,
        info.triangles,
        info.size_in_votv_cm[0],
        info.size_in_votv_cm[1],
        info.size_in_votv_cm[2]
    );
    for h in &info.hints {
        println!("hint: {}", h.message);
    }
    let mut opts = ConvertOptions::default(); // bind these fields to your form widgets
    opts.humanoid = info.skins > 0; // sensible pre-selection
    opts.drop_transparent = info.any_transparent;
    opts.validate().expect("options"); // cheap; call on every edit if you like

    // 2. The user pressed Convert: run it off the UI thread.
    let (tx, rx) = mpsc::channel::<Msg>();
    let cancel = CancelToken::new(); // keep a clone for the Cancel button
    let (txp, txl) = (tx.clone(), tx.clone());
    let hooks = Hooks::default()
        .with_cancel(cancel.clone())
        .with_progress(move |p| {
            let _ = txp.send(Msg::Progress(p.clone()));
        })
        .with_diagnostics(move |d| {
            let _ = txl.send(Msg::Log(d.clone()));
        });
    opts.output_dir = std::env::temp_dir();
    let worker = thread::spawn(move || {
        let r = convert_with(&opts, &input, &hooks);
        let _ = tx.send(Msg::Finished(r));
    });

    // 3. The UI loop drains the channel (an egui/iced app does this once per frame).
    for msg in rx {
        match msg {
            Msg::Progress(p) => println!(
                "[{:>3.0}%] {:?}: {}",
                p.fraction * 100.0,
                p.stage,
                p.message
            ),
            Msg::Log(d) => println!("  ({:?}/{}) {}", d.severity, d.code, d.message),
            Msg::Finished(Ok(r)) => {
                println!("done -> {}", r.model_dir.display());
                break;
            }
            Msg::Finished(Err(e)) => {
                println!("failed [{}]: {e}", e.kind());
                break;
            }
        }
    }
    worker.join().unwrap();
}
