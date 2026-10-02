use crate::model::{Agent, Status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The tool the source reads from isn't installed anywhere we look
    NotFound(String),
    /// It ran, and failed or answered with something we couldn't read
    Failed(String),
}

/// Where agents come from. Blocking: an implementation may start processes or read files.
pub trait DataSource: Send + Sync {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError>;

    /// The statuses this source can ever produce. The settings still list all five; the
    /// rest are marked as never appearing with this source.
    fn emitted_statuses(&self) -> &'static [Status];
}
