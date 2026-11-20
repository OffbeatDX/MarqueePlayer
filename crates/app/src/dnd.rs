//! Drag and drop between the page and the queue. Payloads are plain text, so they are
//! easy to read in logs and tests: `mp:album:<id>`, `mp:track:<ctx>:<index>` (a row of
//! a track list on screen) and `mp:queue:<index>` (a queue row being moved).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Album(i64),
    Track { ctx: String, index: usize },
    QueueRow(usize),
}

impl Payload {
    pub fn encode(&self) -> String {
        match self {
            Payload::Album(id) => format!("mp:album:{id}"),
            Payload::Track { ctx, index } => format!("mp:track:{ctx}:{index}"),
            Payload::QueueRow(i) => format!("mp:queue:{i}"),
        }
    }

    pub fn decode(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("mp:")?;
        let (kind, rest) = rest.split_once(':')?;
        match kind {
            "album" => rest.parse().ok().map(Payload::Album),
            "queue" => rest.parse().ok().map(Payload::QueueRow),
            "track" => {
                let (ctx, index) = rest.rsplit_once(':')?;
                Some(Payload::Track { ctx: ctx.to_owned(), index: index.parse().ok()? })
            }
            _ => None,
        }
    }
}

/// Where a queue or playlist row dragged from `from` and dropped at insertion point `at` (an index
/// in the list before the move, as the drop line shows it) ends up after the move.
pub fn move_target(from: usize, at: usize) -> usize {
    if at > from { at - 1 } else { at }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_round_trip() {
        for p in [
            Payload::Album(42),
            Payload::Track { ctx: "album".into(), index: 3 },
            Payload::Track { ctx: "search".into(), index: 0 },
            Payload::QueueRow(7),
        ] {
            assert_eq!(Payload::decode(&p.encode()), Some(p));
        }
        assert_eq!(Payload::decode("hello"), None);
        assert_eq!(Payload::decode("mp:album:x"), None);
        assert_eq!(Payload::decode("mp:other:1"), None);
    }

    #[test]
    fn moving_down_accounts_for_the_removed_row() {
        // Rows a b c d; drag a (0) to the line before d (3): it lands at index 2.
        assert_eq!(move_target(0, 3), 2);
        // Drag d (3) to the line before a (0).
        assert_eq!(move_target(3, 0), 0);
        // Dropped on its own line, above or below itself: stays.
        assert_eq!(move_target(2, 2), 2);
        assert_eq!(move_target(2, 3), 2);
    }
}
