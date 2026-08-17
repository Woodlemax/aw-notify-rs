use std::fmt;

use crate::SessionHistory;

pub trait HistoryStore: Send {
    fn append(&mut self, session: &SessionHistory) -> Result<(), HistoryError>;
    fn load_all(&mut self) -> Result<Vec<SessionHistory>, HistoryError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryError(pub String);

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HistoryError {}
