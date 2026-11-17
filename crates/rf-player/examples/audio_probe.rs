//! Opens the default audio output, plays 100 ms of silence and reports the rendered frames.
//! Used to check CoreAudio without launching DirectLook.
fn main() {
    let mut sink = match rf_player::wasapi_sink() {
        Ok(s) => s,
        Err(e) => {
            println!("no sink: {e:#}");
            return;
        }
    };
    println!("sink: {} Hz, {} channels", sink.sample_rate(), sink.channels());
    sink.pause(false);
    let frames = 4_800usize;
    let silence = vec![0.0f32; frames * sink.channels() as usize];
    let mut wrote = 0;
    for _ in 0..20 {
        let n = sink.write(&silence);
        wrote += n;
        if n < frames {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    println!("wrote {wrote} frames, played {}", sink.played_frames());
    sink.pause(true);
}
