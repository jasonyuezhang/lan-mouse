#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod native {
    use input_emulation::{Backend, InputEmulation};
    use input_event::{BTN_LEFT, BUTTON_ADOPT_FILE_DRAG, Event, PointerEvent};
    use std::{path::PathBuf, time::Duration};

    #[tokio::main(flavor = "current_thread")]
    pub async fn main() {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("input_capture=debug"),
        )
        .init();
        let mut emulation = InputEmulation::new(Some(Backend::MacOs)).await.unwrap();
        emulation.create(98765).await;
        let result = run(&mut emulation).await;
        emulation.terminate().await;
        result.unwrap();
    }
    async fn run(e: &mut InputEmulation) -> Result<(), Box<dyn std::error::Error>> {
        let root = PathBuf::from(std::env::var("LAN_MOUSE_NATIVE_DRAG_TEST_DIR")?);
        let request = root.join("request.json");
        let _ = std::fs::remove_file(&request);
        let _ = std::fs::remove_file(root.join("result"));
        std::fs::write(root.join("start"), "start")?;
        let start = std::time::Instant::now();
        while !request.exists() {
            if start.elapsed() > Duration::from_secs(5) {
                return Err("probe did not prepare its window".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let r: serde_json::Value = serde_json::from_slice(&std::fs::read(&request)?)?;
        let r = &r["BeginNativeFileDrag"];
        let n = |key: &str| r[key].as_i64().unwrap() as i32;
        let begin = || input_capture::begin_native_file_drag(n("pid"), n("window"), n("x"), n("y"));
        assert!(!begin(), "a released button must not start a drag");
        e.consume(
            Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_LEFT,
                state: BUTTON_ADOPT_FILE_DRAG,
            }),
            98765,
        )
        .await?;
        assert!(!input_capture::begin_native_file_drag(
            n("pid"),
            0,
            n("x"),
            n("y")
        ));
        let started = std::time::Instant::now();
        while !begin() {
            if started.elapsed() > Duration::from_secs(2) {
                return Err("relay did not cover its click point".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!begin(), "startup retries must not send another press");
        while !root.join("started").exists() {
            if started.elapsed() > Duration::from_secs(2) {
                return Err("native mouse-down did not start the relay".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let target: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("target.json"))?)?;
        let dx = (target["x"].as_f64().unwrap() - f64::from(n("x"))) / 60.;
        let dy = (target["y"].as_f64().unwrap() - f64::from(n("y"))) / 60.;
        for _ in 0..60 {
            e.consume(
                Event::Pointer(PointerEvent::Motion { time: 0, dx, dy }),
                98765,
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(16)).await;
            assert!(
                !root.join("finished").exists(),
                "the drag ended before release"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cancel = std::env::args().any(|arg| arg == "--cancel");
        if cancel {
            input_capture::cancel_source_file_drag(n("pid"));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        e.consume(
            Event::Pointer(PointerEvent::Button {
                time: 0,
                button: BTN_LEFT,
                state: 0,
            }),
            98765,
        )
        .await?;
        assert!(
            !input_event::macos::claim_file_drag_button(),
            "release must invalidate pending native startup"
        );
        tokio::time::sleep(Duration::from_millis(1000)).await;
        if cancel {
            assert!(!root.join("result").exists());
            assert_eq!(std::fs::read_to_string(root.join("finished"))?, "cancelled");
            println!("PASS: cancellation ends the native session without delivering a file");
            return Ok(());
        }
        let result = std::fs::read_to_string(root.join("result"))?;
        assert_eq!(result, "accepted");
        println!(
            "PASS: native destination accepted the fixture via the production drag handoff and emulation"
        );
        assert_eq!(std::fs::read_to_string(root.join("finished"))?, "copied");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn main() {
    native::main();
}
