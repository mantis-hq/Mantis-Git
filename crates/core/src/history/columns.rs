//! The columns of the commit graph and the order they stand in. The order is the user's:
//! it is kept in the app's `state` file, so it must survive a file that was edited by hand
//! or written by another version, naming columns twice, leaving some out or inventing new.

/// How many columns there are.
pub const COUNT: usize = 6;

/// One column of a commit row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Refs,
    Graph,
    Subject,
    Hash,
    Author,
    Date,
}

impl Column {
    /// Every column, in the order a fresh install shows them.
    pub const ALL: [Column; COUNT] = [
        Column::Refs,
        Column::Graph,
        Column::Subject,
        Column::Hash,
        Column::Author,
        Column::Date,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Column::Refs => "refs",
            Column::Graph => "graph",
            Column::Subject => "subject",
            Column::Hash => "hash",
            Column::Author => "author",
            Column::Date => "date",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|column| column.key() == key.trim())
    }

    /// The word in the column's header.
    pub fn label(self) -> &'static str {
        match self {
            Column::Refs => "Refs",
            Column::Graph => "Graph",
            Column::Subject => "Subject",
            Column::Hash => "Hash",
            Column::Author => "Author",
            Column::Date => "Date",
        }
    }
}

/// The columns, each exactly once, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnOrder([Column; COUNT]);

impl Default for ColumnOrder {
    fn default() -> Self {
        Self(Column::ALL)
    }
}

impl ColumnOrder {
    pub fn columns(&self) -> [Column; COUNT] {
        self.0
    }

    /// Reads a comma separated list of keys. Unknown and repeated names are skipped and
    /// columns the list leaves out follow in their usual order, so nothing ever vanishes.
    pub fn parse(text: &str) -> Self {
        let mut order = Vec::with_capacity(Column::ALL.len());
        for column in text.split(',').filter_map(Column::from_key) {
            if !order.contains(&column) {
                order.push(column);
            }
        }
        for column in Column::ALL {
            if !order.contains(&column) {
                order.push(column);
            }
        }
        Self(order.try_into().expect("every column is in the list once"))
    }

    pub fn render(&self) -> String {
        let keys: Vec<&str> = self.0.iter().map(|column| column.key()).collect();
        keys.join(",")
    }

    /// Moves `column` to where `target` stands, the ones between shifting over by one.
    pub fn move_to(&mut self, column: Column, target: Column) {
        let (Some(from), Some(to)) = (self.position(column), self.position(target)) else {
            return;
        };
        let mut list = self.0.to_vec();
        let moved = list.remove(from);
        list.insert(to, moved);
        self.0 = list.try_into().expect("moving keeps the length");
    }

    fn position(&self, column: Column) -> Option<usize> {
        self.0.iter().position(|&other| other == column)
    }
}

#[cfg(test)]
mod tests {
    use super::{Column, ColumnOrder};

    #[test]
    fn the_default_order_is_the_one_the_graph_always_had() {
        assert_eq!(
            ColumnOrder::default().render(),
            "refs,graph,subject,hash,author,date"
        );
    }

    #[test]
    fn dropping_on_a_column_takes_its_place() {
        let mut order = ColumnOrder::default();
        order.move_to(Column::Date, Column::Refs);
        assert_eq!(order.render(), "date,refs,graph,subject,hash,author");
        order.move_to(Column::Date, Column::Author);
        assert_eq!(order.render(), "refs,graph,subject,hash,author,date");
    }

    #[test]
    fn a_damaged_list_still_has_every_column_once() {
        let order = ColumnOrder::parse("date, nonsense,date,subject,,");
        assert_eq!(order.render(), "date,subject,refs,graph,hash,author");
        assert_eq!(ColumnOrder::parse(""), ColumnOrder::default());
    }

    #[test]
    fn what_is_written_reads_back() {
        let mut order = ColumnOrder::default();
        order.move_to(Column::Author, Column::Graph);
        assert_eq!(ColumnOrder::parse(&order.render()), order);
    }
}
