/// What the mouse pointer is over, registered while drawing: an item of a
/// CatTube screen (`Tube(row, column)`, `usize::MAX` for the header), a
/// key of the search keyboard or a search suggestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tube(usize, usize),
    /// A key of the search keyboard (row, column).
    Key(usize, usize),
    /// A search suggestion.
    Suggestion(usize),
}
