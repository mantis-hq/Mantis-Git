use super::{FileView, Row};
use gitclient_core::repo::conflict::Piece;

fn lines(prefix: &str, count: usize) -> String {
    (1..=count).map(|n| format!("{prefix} {n}\n")).collect()
}

#[test]
fn both_versions_of_the_whole_file_are_laid_out_side_by_side() {
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
    assert_eq!(view.len(), 20 + 1 + 3 + 20);
    assert_eq!(view.conflicts, [(20, 4)]);
    assert_eq!(view.agreed, [20]);
    assert!(matches!(
        view.rows[20],
        Row::Header {
            ordinal: 0,
            line: 21
        }
    ));
    // Inside the conflict each side is numbered in its own version of the file, and the
    // side with fewer lines runs out before the other.
    assert!(matches!(
        &view.rows[23],
        Row::Pair {
            mine: None,
            other: Some((23, _)),
            ..
        }
    ));
    // Below it, the line both versions have shows in both, at its number in each.
    assert!(matches!(
        &view.rows[24],
        Row::Context {
            mine: 23,
            other: 24,
            ..
        }
    ));
}

#[test]
fn each_conflict_header_is_found_by_its_row() {
    let conflict = || Piece::Conflict {
        mine: "a\n".to_owned(),
        other: "b\n".to_owned(),
    };
    let pieces = [
        Piece::Text(lines("above", 10)),
        conflict(),
        Piece::Text(lines("between", 10)),
        conflict(),
    ];
    let view = FileView::new(&pieces);
    assert_eq!(view.header_row(0), 10);
    assert_eq!(view.header_row(1), 22);
    assert_eq!(view.agreed, [10, 10]);

    // Counting from a row in view, with the header at that row counted or not.
    assert_eq!(view.conflicts_above(10, false), 0);
    assert_eq!(view.conflicts_above(10, true), 1);
    assert_eq!(view.conflicts_above(30, false), 2);
}

#[test]
fn the_list_and_the_result_are_lined_up_at_each_conflicts_edges() {
    let pieces = [
        Piece::Text(lines("above", 10)),
        Piece::Conflict {
            mine: lines("mine", 2),
            other: lines("other", 2),
        },
        Piece::Text(lines("below", 10)),
    ];
    let view = FileView::new(&pieces);
    // Three rows in the list; settled with one line in the result.
    let settled_in_one_line = 10..11;
    let anchors = view.anchors(std::slice::from_ref(&settled_in_one_line));
    assert_eq!(anchors, [(0., 0.), (10., 10.), (13., 11.)]);
}
