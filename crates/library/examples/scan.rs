//! `cargo run -p library --release --example scan -- <db> <art-dir> <root>... [--changed <path>]...`
//! Scans into the given database and prints a report. Music folders are only read.
//! With `--changed`, scans as the folder watcher would after those paths changed (a
//! partial scan of their folders; see `library::scan::scopes`).

fn main() {
    let mut args = std::env::args().skip(1);
    let db = std::path::PathBuf::from(args.next().expect("db path"));
    let art = std::path::PathBuf::from(args.next().expect("art dir"));
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    let mut changed: Vec<std::path::PathBuf> = Vec::new();
    while let Some(a) = args.next() {
        if a == "--changed" {
            changed.push(args.next().expect("--changed <path>").into());
        } else {
            roots.push(a.into());
        }
    }
    let mut conn = library::db::open(&db).unwrap();
    let cache = library::ArtCache::new(art);
    let only = (!changed.is_empty()).then(|| library::scan::scopes(&changed, &roots)).flatten();
    if !changed.is_empty() {
        println!("changed {changed:?} -> scan {only:?}");
    }
    let opts = library::scan::Options { only, ..Default::default() };
    let report = library::scan::scan_with(&mut conn, &cache, &roots, opts, &|p| {
        if p.done == p.total {
            eprintln!("{:?} {}/{}", p.phase, p.done, p.total);
        }
    })
    .unwrap();
    let (albums, tracks) = library::query::counts(&conn).unwrap();
    println!(
        "files {} read {} removed {} moved {} albums_changed {} errors {} partial {} in {:.1} ms -> {albums} albums, {tracks} tracks",
        report.files,
        report.read,
        report.removed,
        report.moved,
        report.albums_changed,
        report.errors.len(),
        report.partial,
        report.seconds * 1000.0
    );
    println!("batches {} longest write {:.1} ms resumed {} interrupted {}", report.batches, report.longest_write * 1000.0, report.resumed, report.interrupted);
    for (p, e) in report.errors.iter().take(10) {
        println!("  error {}: {e}", p.display());
    }
    let no_art: i64 = conn.query_row("SELECT COUNT(*) FROM albums WHERE art IS NULL", [], |r| r.get(0)).unwrap();
    println!("albums without art: {no_art}");
}
