//! List the render devices, as Settings > Output shows them. Opens nothing.
//!
//!     cargo run --release -p player-core --example devices

fn main() {
    match player_core::output::devices() {
        Ok(list) => {
            for d in list {
                println!("{}{}  [{}]", d.name, if d.default { " (default)" } else { "" }, d.id);
            }
        }
        Err(e) => eprintln!("cannot list devices: {e}"),
    }
}
