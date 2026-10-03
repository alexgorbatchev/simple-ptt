    // README demo, inserted by the readme-demo skill's record script before
    // `let _keep = (tuner, window, timer);` in `tuner::run`. Never commit it.
    //
    // A developer dictates a prompt for their coding agent, corrects it midway
    // with the correction key, and keeps dictating. The demo stops the tuner's
    // own timer and drives the overlay through the states the app uses:
    // recording, the correction spoken while recording, the correction applied
    // (transforming, with the inline preview), recording again, and the pop out.
    if std::env::var("VIEW_PROBE").is_ok() {
        use super::view_probe::{overlay_panel, run_script};

        /// Seconds per narrated word (about 2.7 words a second), and how many
        /// of the latest words stay interim while speaking goes on.
        const WORD_SECONDS: f64 = 0.375;
        const INTERIM_WORDS: usize = 3;
        const FIRST: &str = "Add retries with exponential backoff to the webhook sender in the notifications crate, and cap it at three attempts.";
        const CORRECTION: &str = "make that five attempts";
        const CORRECTED: &str = "Add retries with exponential backoff to the webhook sender in the notifications crate, and cap it at five attempts.";
        const SECOND: &str = "Then log every failure with its status code, and add a test for the timeout path.";

        /// Seconds after the timeline starts: the overlay shows, and the
        /// phases that follow, each relative to `SHOW`.
        const SHOW: f64 = 2.5;
        const FIRST_AT: f64 = 0.6;
        const CORRECTION_AT: f64 = 8.2;
        const TRANSFORM_AT: f64 = 10.2;
        const PREVIEW_AT: f64 = 10.8;
        const RESUME_AT: f64 = 11.9;
        const SECOND_AT: f64 = 12.1;
        const HIDE_AT: f64 = 18.9;
        /// The timeline's capture starts here; screencapture takes about a
        /// second to begin recording, so it starts well before `SHOW`.
        const CAPTURE_AT: f64 = 1.0;

        /// `words` narrated `elapsed` seconds in, after `prefix`: the text and
        /// where its interim words start.
        fn narrated(prefix: &str, words: &str, elapsed: f64) -> (String, Option<usize>) {
            let words: Vec<&str> = words.split_whitespace().collect();
            let speaking = words.len() as f64 * WORD_SECONDS;
            let (spoken, finals) = if elapsed >= speaking {
                (words.len(), words.len())
            } else {
                let spoken = ((elapsed.max(0.0) / WORD_SECONDS) as usize + 1).min(words.len());
                (spoken, spoken.saturating_sub(INTERIM_WORDS))
            };
            let mut text = prefix.to_owned();
            let mut provisional_start = None;
            for (index, word) in words.iter().take(spoken).enumerate() {
                if index == finals {
                    provisional_start = Some(text.len());
                }
                text.push_str(word);
                text.push(' ');
            }
            (text, provisional_start)
        }

        /// Whether the developer is speaking `at` seconds after `SHOW`.
        fn speaking(at: f64) -> bool {
            let words = |text: &str| text.split_whitespace().count() as f64 * WORD_SECONDS;
            (FIRST_AT..FIRST_AT + words(FIRST)).contains(&at)
                || (CORRECTION_AT..CORRECTION_AT + words(CORRECTION)).contains(&at)
                || (SECOND_AT..SECOND_AT + words(SECOND)).contains(&at)
        }

        let pop_seconds = state.overlay.glass_tuning().pop_seconds;
        let end = SHOW + HIDE_AT + pop_seconds + 1.0;
        let screen_height = NSScreen::mainScreen(mtm).expect("screen").frame().size.height;
        let capture = match (std::env::var("DEMO_CAPTURE").ok(), std::env::var("DEMO_RECT").ok()) {
            (Some(path), Some(rect)) => Some((path, rect, end - CAPTURE_AT)),
            _ => None,
        };
        let setup = state.clone();
        let demo_window = window.clone();
        let demo_timer = timer.clone();
        let events: Vec<(f64, Box<dyn Fn()>)> = vec![
            (0.3, Box::new(move || {
                demo_timer.invalidate();
                demo_window.orderOut(None);
                setup.overlay.hide();
                setup.overlay.pin_to_top(None);
            })),
            (CAPTURE_AT, Box::new(move || {
                if let Some((path, rect, seconds)) = capture.as_ref() {
                    std::process::Command::new("/usr/sbin/screencapture")
                        .args(["-x", "-v", &format!("-V{seconds:.1}"), &format!("-R{rect}"), path.as_str()])
                        .spawn()
                        .expect("screencapture starts");
                }
            })),
        ];

        let demo = state.clone();
        let tick = Cell::new(0_u64);
        let hidden = Cell::new(false);
        run_script(events, 0.075, end, move |t| {
            let at = t - SHOW;
            if at >= 0.0 && !hidden.get() {
                if at >= HIDE_AT {
                    hidden.set(true);
                    demo.overlay.hide();
                    eprintln!("demo-timing hide={t:.3} pop_seconds={pop_seconds:.3}");
                } else {
                    if tick.get() == 0 {
                        eprintln!("demo-timing show={t:.3}");
                    }
                    let step = tick.get() + 1;
                    tick.set(step);
                    // The voice's 6 s silence (`tick % 80 >= 60`) never falls
                    // inside a phrase; its breaths between words stay.
                    let level_db = if speaking(at) { voice_level_db(step % 60) } else { VOICE_ROOM_DB };
                    demo.publish_voice(step, level_db);
                    let mic = MicMeterSnapshot {
                        clip_event_counter: 0,
                        level: normalized_meter_value(normalize_meter_amplitude(level_db)),
                        peak: normalized_meter_value(normalize_meter_amplitude(level_db + VOICE_CREST_DB)),
                        mic_active: true,
                    };

                    let texts = AppState::new();
                    let (main, provisional_start) = if at < CORRECTION_AT {
                        narrated("", FIRST, at - FIRST_AT)
                    } else if at < RESUME_AT {
                        (format!("{FIRST} "), None)
                    } else {
                        narrated(&format!("{CORRECTED} "), SECOND, at - SECOND_AT)
                    };
                    let main = if at < FIRST_AT { String::new() } else { main };
                    texts.set_live_overlay_text(main, provisional_start);
                    let (correction, correction_provisional) = if at < TRANSFORM_AT {
                        narrated("", CORRECTION, at - CORRECTION_AT)
                    } else if at < PREVIEW_AT {
                        (String::new(), None)
                    } else {
                        (format!("{CORRECTED} "), None)
                    };
                    texts.set_live_overlay_correction_text(correction, correction_provisional);
                    let correction_held = (CORRECTION_AT..TRANSFORM_AT).contains(&at);
                    let applying = (TRANSFORM_AT..RESUME_AT).contains(&at);
                    let app_state = if applying { STATE_TRANSFORMING } else { STATE_RECORDING };
                    demo.overlay.update(
                        mtm,
                        app_state,
                        false,
                        &texts.overlay_text_snapshot(),
                        "",
                        &texts.overlay_correction_text_snapshot(),
                        correction_held,
                        1.0,
                        mic,
                        true,
                    );
                }
            }
            if let Some(panel) = overlay_panel() {
                let frame = panel.frame();
                eprintln!(
                    "demo-panel t={t:.2} x={:.1} top={:.1} w={:.1} h={:.1} visible={}",
                    frame.origin.x,
                    screen_height - (frame.origin.y + frame.size.height),
                    frame.size.width,
                    frame.size.height,
                    panel.isVisible()
                );
            }
        });
    }
