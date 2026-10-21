/// Something loaded in the background: not started, loading, loaded, or
/// failed with a message for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scan<T> {
    Idle,
    Busy,
    Done(Vec<T>),
    Failed(String),
}

impl<T> Scan<T> {
    pub fn items(&self) -> &[T] {
        match self {
            Scan::Done(v) => v,
            _ => &[],
        }
    }
}
