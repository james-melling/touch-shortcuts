//! Diagnostic: `cargo run -p tsp-mac --example send -- "<combo>" [repeat]`
//! Sends the combo through the real sink to the app in front 2 s from now.
#[cfg(target_os = "macos")]
fn main() {
    use tsp_core::combo::{parse_combo, press_events};
    use tsp_core::executor::{Foreground, KeySink};
    let args: Vec<String> = std::env::args().skip(1).collect();
    let combo = parse_combo(args.first().map(String::as_str).unwrap_or("Num1")).expect("combo");
    let repeat: usize = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(1);
    println!("post permission: {}", tsp_mac::request_post_access());
    std::thread::sleep(std::time::Duration::from_secs(2));
    let target = tsp_mac::MacForeground.current();
    println!("target: {} (pid {}, pad: {})", target.title, target.id, target.is_pad);
    for _ in 0..repeat {
        let n = tsp_mac::MacSink.send(&press_events(&combo));
        println!("posted {n} events");
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}
