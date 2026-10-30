//! The app's data source: herdr's agents and the Claude Code hook sessions, side by side.

use agent_core::{Agent, DataSource, SourceError, Status};

pub struct MergedSource {
    herdr: Box<dyn DataSource>,
    hooks: Box<dyn DataSource>,
    emitted: &'static [Status],
}

impl MergedSource {
    pub fn new(herdr: Box<dyn DataSource>, hooks: Box<dyn DataSource>) -> MergedSource {
        let (a, b) = (herdr.emitted_statuses(), hooks.emitted_statuses());
        let union: Vec<Status> = Status::ALL
            .into_iter()
            .filter(|s| a.contains(s) || b.contains(s))
            .collect();
        // One source per process, built once at startup, so the leak is bounded.
        let emitted = Box::leak(union.into_boxed_slice());
        MergedSource {
            herdr,
            hooks,
            emitted,
        }
    }
}

impl DataSource for MergedSource {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        let herdr = self.herdr.fetch();
        // HookSource never fails; a failing hooks source adds nothing rather than
        // hiding herdr's agents.
        let hooks = self.hooks.fetch().unwrap_or_default();
        match herdr {
            Ok(mut agents) => {
                agents.extend(hooks);
                Ok(agents)
            }
            Err(SourceError::NotFound(_)) if !hooks.is_empty() => Ok(hooks),
            Err(e) => Err(e),
        }
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        self.emitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        result: Result<Vec<Agent>, SourceError>,
        emitted: &'static [Status],
    }

    impl DataSource for Fake {
        fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
            self.result.clone()
        }
        fn emitted_statuses(&self) -> &'static [Status] {
            self.emitted
        }
    }

    fn agent(name: &str) -> Agent {
        Agent {
            status: Status::Idle,
            name: name.into(),
            workspace: String::new(),
            title: String::new(),
        }
    }

    fn merged(
        herdr: Result<Vec<Agent>, SourceError>,
        hooks: Result<Vec<Agent>, SourceError>,
    ) -> Result<Vec<Agent>, SourceError> {
        let fake = |result| {
            Box::new(Fake {
                result,
                emitted: &[],
            })
        };
        MergedSource::new(fake(herdr), fake(hooks)).fetch()
    }

    fn not_found() -> SourceError {
        SourceError::NotFound("herdr not found".into())
    }

    #[test]
    fn herdr_agents_come_first_then_hook_sessions() {
        let got = merged(Ok(vec![agent("h")]), Ok(vec![agent("c")]));
        assert_eq!(got, Ok(vec![agent("h"), agent("c")]));
    }

    #[test]
    fn without_herdr_the_hook_sessions_are_shown() {
        let got = merged(Err(not_found()), Ok(vec![agent("c")]));
        assert_eq!(got, Ok(vec![agent("c")]));
    }

    #[test]
    fn without_herdr_or_sessions_herdr_is_reported_missing() {
        assert_eq!(merged(Err(not_found()), Ok(vec![])), Err(not_found()));
    }

    #[test]
    fn a_herdr_failure_is_reported_even_with_sessions() {
        let failed = SourceError::Failed("boom".into());
        let got = merged(Err(failed.clone()), Ok(vec![agent("c")]));
        assert_eq!(got, Err(failed));
    }

    fn emitted(herdr: &'static [Status], hooks: &'static [Status]) -> &'static [Status] {
        let fake = |emitted| {
            Box::new(Fake {
                result: Ok(vec![]),
                emitted,
            })
        };
        MergedSource::new(fake(herdr), fake(hooks)).emitted_statuses()
    }

    #[test]
    fn emitted_statuses_are_the_union() {
        let hooks = &[Status::Done, Status::Idle, Status::Working, Status::Blocked];
        assert_eq!(emitted(&Status::ALL, hooks), Status::ALL);
    }

    #[test]
    fn emitted_statuses_follow_the_status_order() {
        assert_eq!(
            emitted(&[Status::Working], &[Status::Done]),
            [Status::Done, Status::Working]
        );
    }
}
