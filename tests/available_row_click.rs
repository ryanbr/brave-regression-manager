//! Headless reproduction of the Available list's clickable note cell.
//!
//! Clicking `+` / the Comments cell reportedly does nothing. The note key
//! chain, the editor call site and window positioning have all been ruled
//! out by reading, which leaves the question of whether the click reaches
//! the label at all through the nesting the virtualised list introduced:
//!
//!     ScrollArea::show_rows -> push_id -> horizontal -> scope(min/max
//!     width) -> Label::sense(click)
//!
//! egui runs fine without a window, so this asks the real widget stack
//! rather than reasoning about it.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect};

const SCREEN: Rect = Rect {
    min: Pos2 { x: 0.0, y: 0.0 },
    max: Pos2 { x: 900.0, y: 600.0 },
};

fn base_input() -> RawInput {
    RawInput { screen_rect: Some(SCREEN), ..Default::default() }
}

/// Lay the list out, then click at `click_at` on the following frame.
/// Returns (rect of the probed row's note label, whether it was clicked).
fn run(rows: usize, probe_row: usize, click_at: Option<Pos2>) -> (Option<Rect>, bool) {
    let ctx = egui::Context::default();
    let mut probed = None;
    let mut clicked = false;

    let frame = |input: RawInput, probed: &mut Option<Rect>, clicked: &mut bool| {
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let row_h = ui.spacing().interact_size.y
                    .max(ui.text_style_height(&egui::TextStyle::Body) + 1.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show_rows(ui, row_h, rows, |ui, range| {
                        for font_id in ui.style_mut().text_styles.values_mut() {
                            font_id.size += 1.0;
                        }
                        for i in range {
                            ui.push_id(i, |ui| {
                                ui.horizontal(|ui| {
                                    ui.scope(|ui| {
                                        ui.set_min_width(60.0);
                                        ui.set_max_width(60.0);
                                        ui.monospace(format!("v1.0.{i}"));
                                    });
                                    ui.scope(|ui| {
                                        ui.set_min_width(28.0);
                                        ui.set_max_width(28.0);
                                        let r = ui.add(
                                            egui::Label::new("+")
                                                .sense(egui::Sense::click()),
                                        );
                                        if i == probe_row {
                                            *probed = Some(r.rect);
                                            if r.clicked() { *clicked = true; }
                                        }
                                    });
                                });
                            });
                        }
                    });
            });
        });
    };

    // Frame 1: lay out and learn where the label is.
    frame(base_input(), &mut probed, &mut clicked);

    if let Some(at) = click_at {
        // Frame 2: hover it.
        let mut i = base_input();
        i.events.push(Event::PointerMoved(at));
        frame(i, &mut probed, &mut clicked);
        // Frame 3: press and release on it.
        let mut i = base_input();
        i.events.push(Event::PointerMoved(at));
        i.events.push(Event::PointerButton {
            pos: at, button: PointerButton::Primary,
            pressed: true, modifiers: Modifiers::default(),
        });
        i.events.push(Event::PointerButton {
            pos: at, button: PointerButton::Primary,
            pressed: false, modifiers: Modifiers::default(),
        });
        frame(i, &mut probed, &mut clicked);
    }
    (probed, clicked)
}

#[test]
fn note_cell_receives_a_click_in_the_first_visible_row() {
    let (rect, _) = run(500, 0, None);
    let rect = rect.expect("row 0 should be laid out");
    let (_, clicked) = run(500, 0, Some(rect.center()));
    assert!(clicked, "click at {:?} did not reach the note label", rect.center());
}

/// The rows egui hands back are a window into a long list; a row further
/// down must behave the same as the first.
#[test]
fn note_cell_receives_a_click_in_a_later_visible_row() {
    let probe = 8;
    let (rect, _) = run(500, probe, None);
    let rect = rect.expect("row 8 should be laid out and visible");
    let (_, clicked) = run(500, probe, Some(rect.center()));
    assert!(clicked, "click at {:?} did not reach row {probe}", rect.center());
}

/// The height passed to show_rows must match what a row actually paints,
/// or the reserved slots and the painted rows drift apart with depth.
#[test]
fn painted_row_height_matches_the_budget() {
    let (r0, _) = run(500, 0, None);
    let (r1, _) = run(500, 1, None);
    let (r5, _) = run(500, 5, None);
    let (a, b, c) = (r0.unwrap(), r1.unwrap(), r5.unwrap());
    let step = b.center().y - a.center().y;
    let step5 = (c.center().y - a.center().y) / 5.0;
    assert!(
        (step - step5).abs() < 0.5,
        "row pitch drifts with depth: 1 row = {step}, averaged over 5 = {step5}"
    );
}
