//! Look one track up on LRCLIB (or `MP_LRCLIB_API`) and print what the app would show:
//!   cargo run --release -p lrclib --example probe -- "<artist>" "<title>" "<album>" <seconds>
//! Sends the given artist, title, album and length; nothing is cached.

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [artist, title, album, secs] = a.as_slice() else {
        eprintln!("usage: probe <artist> <title> <album> <seconds>");
        std::process::exit(2);
    };
    let t = lrclib::TrackInfo { artist: artist.clone(), title: title.clone(), album: album.clone(), duration: secs.parse().unwrap_or(0.0) };
    let root = std::env::var("MP_LRCLIB_API").unwrap_or_else(|_| "https://lrclib.net".into());
    let c = lrclib::Client::new(root, concat!("MarqueePlayer/", env!("CARGO_PKG_VERSION"), " (lyrics probe)"));
    match c.lookup(&t, &[]) {
        Ok(lrclib::Answer::Found { id, lyrics }) => {
            println!("found record {id}: {} lines, synced {}", lyrics.lines.len(), lyrics.synced);
            for l in lyrics.lines.iter().take(4) {
                println!("  {:?} {}", l.time_ms, l.text);
            }
        }
        Ok(a) => println!("{a:?}"),
        Err(e) => println!("error: {e}"),
    }
}
