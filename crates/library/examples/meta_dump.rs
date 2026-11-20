//! Print what the metadata viewer reads from a file: its tag blocks, each field with every
//! block's say on it, and its pictures. Read-only.
//!
//!     cargo run --release -p library --example meta_dump -- "M:\Music\Album\01 Track.flac"

fn main() {
    for arg in std::env::args().skip(1) {
        let path = std::path::Path::new(&arg);
        println!("== {}", path.display());
        let tags = match library::meta::read(path, library::meta::ReadOptions::ALL) {
            Ok(t) => t,
            Err(e) => {
                println!("   could not be read: {e}");
                continue;
            }
        };
        println!("   blocks: {:?}", tags.blocks.iter().map(|b| b.name()).collect::<Vec<_>>());
        for f in library::meta::fields(&tags) {
            let mark = if f.differs() { "  (blocks differ)" } else { "" };
            println!("   [{}] {}{mark}", f.group().title(), f.label());
            for s in &f.sources {
                let vals: Vec<String> = s.values.iter().map(|v| if v.chars().count() > 70 { format!("{}… ({} chars)", v.chars().take(70).collect::<String>(), v.chars().count()) } else { v.clone() }).collect();
                println!("       {} {} = {:?}{}", tags.blocks[s.block].name(), s.key, vals, s.binary.map(|b| format!(" binary {b} bytes")).unwrap_or_default());
            }
        }
        for p in &tags.pictures {
            println!("   picture: {} {} {:?} {} bytes {:?} thumb {:?}", p.kind, p.format, p.size, p.bytes, p.description, p.thumb.as_ref().map(|t| (t.width, t.height)));
        }
        if tags.dropped > 0 {
            println!("   {} items dropped", tags.dropped);
        }
    }
}
