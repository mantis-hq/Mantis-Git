use super::{FileView, Row, Visible};
use gitclient_core::repo::conflict::Piece;

fn lines(prefix: &str, count: usize) -> String {
    (1..=count).map(|n| format!("{prefix} {n}\n")).collect()
}

#[test]
fn a_file_is_laid_out_with_context_around_each_conflict_and_gaps_between() {
    let pieces = [
        Piece::Text(lines("above", 20)),
        Piece::Conflict {
            mine: lines("mine", 2),
            other: lines("other", 3),
        },
        Piece::Text(lines("below", 20)),
    ];
    let view = FileView::new(&pieces);

    // Every line of the file, and the conflict as a header and three rows of lines.
    assert_eq!(view.whole.len(), 20 + 1 + 3 + 20);
    assert_eq!(view.conflicts, [(20, 4)]);
    assert!(matches!(
        view.whole[20],
        Row::Header {
            ordinal: 0,
            line: 21
        }
    ));
    // The side with fewer lines runs out before the other.
    assert!(matches!(
        &view.whole[23],
        Row::Pair {
            mine: None,
            other: Some(_),
            ..
        }
    ));

    // Only the changes: three lines of context each way and a note for what was left out.
    assert_eq!(view.len(true), 44);
    assert_eq!(view.len(false), 1 + 3 + 4 + 3 + 1);
    assert!(matches!(view.changes[0], Visible::Gap(17)));
    assert!(matches!(
        view.changes[view.changes.len() - 1],
        Visible::Gap(17)
    ));
    assert!(matches!(view.visible(false, 1), Visible::Row(17)));
    assert!(matches!(view.visible(true, 5), Visible::Row(5)));
}

#[test]
fn a_conflict_near_the_edges_has_no_gap_where_nothing_was_left_out() {
    let pieces = [
        Piece::Conflict {
            mine: "a\n".to_owned(),
            other: "b\n".to_owned(),
        },
        Piece::Text(lines("below", 2)),
    ];
    let view = FileView::new(&pieces);
    assert!(
        view.changes
            .iter()
            .all(|row| matches!(row, Visible::Row(_)))
    );
    assert_eq!(view.len(false), view.len(true));
}
