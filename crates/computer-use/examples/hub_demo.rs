//! Join N agents to the hub and move their cursors, to see how the shared
//! overlay looks: `cargo run --example hub_demo -- <computer-use-mcp> [N]`.
//! Each agent glides to the middle of its part of the screen and clicks.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use computer_use::config::OverlayConfig;
use computer_use::overlay::hub::JoinOptions;
use computer_use::overlay::{Cmd, Keys, Launcher, Overlay, Status};
use computer_use::types::Rect;

fn main() {
    let mut args = std::env::args().skip(1);
    let program = args.next().expect("the computer-use-mcp program");
    let n: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(3);
    let home = std::env::temp_dir().join("cu-hub-demo");
    let launcher = Launcher::helper(program);
    let screen = Rect::new(0.0, 0.0, 1280.0, 800.0);
    let cfg = OverlayConfig {
        enabled: true,
        ..OverlayConfig::default()
    };
    let keys = Keys {
        stop: "ctrl+alt+escape".into(),
        settings: String::new(),
    };
    let mut agents = Vec::new();
    for i in 0..n {
        let opts = JoinOptions {
            port: 47_399,
            home: home.clone(),
            client: if i % 2 == 0 { "claude-code" } else { "codex" }.into(),
            want: None,
            screen: Some(screen),
            release_secs: 0,
        };
        let o = Overlay::join_hub(
            &launcher,
            &opts,
            &cfg,
            &keys,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .expect("join the hub");
        println!("joined as agent {}", o.hub().unwrap().agent());
        // At work: only agents at work have a part of the screen.
        o.send(&Cmd::Begin);
        agents.push(o);
    }
    std::thread::sleep(Duration::from_millis(500));
    for o in &mut agents {
        let link = o.hub().unwrap().clone();
        let (area, _) = link.region();
        let r = area.unwrap_or(screen);
        o.send(&Cmd::Begin);
        o.send(&Cmd::Target {
            rect: Some([r.x + 40.0, r.y + 60.0, r.width - 80.0, r.height - 120.0]),
        });
        o.point(
            r.x + r.width / 2.0,
            r.y + r.height / 2.0,
            true,
            Duration::from_millis(250),
        );
        println!("agent {}: area {:?}", link.agent(), area);
    }
    if let Some(o) = agents.last() {
        o.send(&Cmd::Status {
            state: Status::Thinking,
        });
    }
    std::thread::sleep(Duration::from_secs(
        std::env::var("HUB_DEMO_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4),
    ));
}
