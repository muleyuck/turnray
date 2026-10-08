/// The app's own status set. Every data source maps whatever it receives onto these five
/// before handing it over, so priority and display never depend on the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Blocked,
    Done,
    Idle,
    Working,
    Unknown,
}

impl Status {
    pub const COUNT: usize = 5;

    /// Also the default priority order (`Priority::default`).
    pub const ALL: [Status; Status::COUNT] = [
        Status::Blocked,
        Status::Done,
        Status::Idle,
        Status::Working,
        Status::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Blocked => "blocked",
            Status::Done => "done",
            Status::Idle => "idle",
            Status::Working => "working",
            Status::Unknown => "unknown",
        }
    }

    /// `None` for anything that isn't one of the five names.
    pub fn parse(s: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|st| st.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub status: Status,
    pub name: String,
    pub workspace: String,
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_round_trips_through_its_name() {
        for st in Status::ALL {
            assert_eq!(Status::parse(st.as_str()), Some(st));
        }
    }

    #[test]
    fn a_name_outside_the_five_does_not_parse() {
        assert_eq!(Status::parse("Blocked"), None);
        assert_eq!(Status::parse(""), None);
        assert_eq!(Status::parse("waiting"), None);
    }
}
