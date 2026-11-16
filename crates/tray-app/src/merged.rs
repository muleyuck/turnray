//! The app's data source: herdr's agents, kitty's and the agents' hook sessions, side by side.

use agent_core::{Agent, DataSource, SourceError, Status};

pub struct MergedSource {
    herdr: Box<dyn DataSource>,
    kitty: Box<dyn DataSource>,
    hooks: Box<dyn DataSource>,
    emitted: &'static [Status],
}

impl MergedSource {
    pub fn new(
        herdr: Box<dyn DataSource>,
        kitty: Box<dyn DataSource>,
        hooks: Box<dyn DataSource>,
    ) -> MergedSource {
        let sources = [&herdr, &kitty, &hooks];
        let union: Vec<Status> = Status::ALL
            .into_iter()
            .filter(|s| sources.iter().any(|src| src.emitted_statuses().contains(s)))
            .collect();
        // One source per process, built once at startup, so the leak is bounded.
        let emitted = Box::leak(union.into_boxed_slice());
        MergedSource {
            herdr,
            kitty,
            hooks,
            emitted,
        }
    }
}

/// Drops the cards of `later` that `earlier` already shows: any identity in common.
/// Cards within `later` are never compared with each other.
fn without_seen(later: Vec<Agent>, earlier: &[Agent]) -> Vec<Agent> {
    later
        .into_iter()
        .filter(|l| {
            !earlier
                .iter()
                .any(|e| l.identities.iter().any(|i| e.identities.contains(i)))
        })
        .collect()
}

impl DataSource for MergedSource {
    fn fetch(&self) -> Result<Vec<Agent>, SourceError> {
        let herdr = self.herdr.fetch();
        // Kitty and hooks never hide herdr's agents: a failing one adds nothing.
        let kitty = self.kitty.fetch().unwrap_or_default();
        let hooks = self.hooks.fetch().unwrap_or_default();
        // The same agent can arrive from several sources (Codex's hook also fires inside
        // herdr or kitty); the earlier source's card wins.
        let (mut agents, herdr_error) = match herdr {
            Ok(agents) => (agents, None),
            Err(e @ SourceError::NotFound(_)) => (Vec::new(), Some(e)),
            Err(e) => return Err(e),
        };
        let kitty = without_seen(kitty, &agents);
        agents.extend(kitty);
        let hooks = without_seen(hooks, &agents);
        agents.extend(hooks);
        match herdr_error {
            Some(e) if agents.is_empty() => Err(e),
            _ => Ok(agents),
        }
    }

    fn emitted_statuses(&self) -> &'static [Status] {
        self.emitted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::Identity;

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
            identities: vec![],
        }
    }

    fn session(name: &str, id: Option<&str>) -> Agent {
        Agent {
            identities: id
                .map(|s| Identity::Session(s.into()))
                .into_iter()
                .collect(),
            ..agent(name)
        }
    }

    #[test]
    fn a_hook_session_that_herdr_also_reports_is_dropped_and_others_stay() {
        let got = merged(
            Ok(vec![session("h", Some("s1"))]),
            Ok(vec![]),
            Ok(vec![
                session("dup", Some("s1")),
                session("other", Some("s2")),
            ]),
        );
        assert_eq!(
            got,
            Ok(vec![session("h", Some("s1")), session("other", Some("s2"))])
        );
    }

    #[test]
    fn a_hook_session_matching_any_herdr_agent_is_dropped() {
        let got = merged(
            Ok(vec![session("h1", Some("s1")), session("h2", Some("s2"))]),
            Ok(vec![]),
            Ok(vec![session("dup", Some("s2"))]),
        );
        assert_eq!(
            got,
            Ok(vec![session("h1", Some("s1")), session("h2", Some("s2"))])
        );
    }

    #[test]
    fn a_hook_session_without_an_id_stays_beside_a_herdr_agent_with_one() {
        let got = merged(
            Ok(vec![session("h", Some("s1"))]),
            Ok(vec![]),
            Ok(vec![session("c", None)]),
        );
        assert_eq!(got, Ok(vec![session("h", Some("s1")), session("c", None)]));
    }

    #[test]
    fn a_hook_session_with_an_id_stays_beside_a_herdr_agent_without_one() {
        let got = merged(
            Ok(vec![session("h", None)]),
            Ok(vec![]),
            Ok(vec![session("c", Some("s1"))]),
        );
        assert_eq!(got, Ok(vec![session("h", None), session("c", Some("s1"))]));
    }

    #[test]
    fn without_herdr_hook_sessions_are_kept_even_with_ids() {
        let got = merged(
            Err(not_found()),
            Ok(vec![]),
            Ok(vec![session("c", Some("s1"))]),
        );
        assert_eq!(got, Ok(vec![session("c", Some("s1"))]));
    }

    fn merged(
        herdr: Result<Vec<Agent>, SourceError>,
        kitty: Result<Vec<Agent>, SourceError>,
        hooks: Result<Vec<Agent>, SourceError>,
    ) -> Result<Vec<Agent>, SourceError> {
        let fake = |result| {
            Box::new(Fake {
                result,
                emitted: &[],
            })
        };
        MergedSource::new(fake(herdr), fake(kitty), fake(hooks)).fetch()
    }

    fn not_found() -> SourceError {
        SourceError::NotFound("herdr not found".into())
    }

    #[test]
    fn herdr_agents_come_first_then_hook_sessions() {
        let got = merged(Ok(vec![agent("h")]), Ok(vec![]), Ok(vec![agent("c")]));
        assert_eq!(got, Ok(vec![agent("h"), agent("c")]));
    }

    #[test]
    fn without_herdr_the_hook_sessions_are_shown() {
        let got = merged(Err(not_found()), Ok(vec![]), Ok(vec![agent("c")]));
        assert_eq!(got, Ok(vec![agent("c")]));
    }

    #[test]
    fn without_herdr_or_sessions_herdr_is_reported_missing() {
        assert_eq!(
            merged(Err(not_found()), Ok(vec![]), Ok(vec![])),
            Err(not_found())
        );
    }

    #[test]
    fn a_herdr_failure_is_reported_even_with_sessions() {
        let failed = SourceError::Failed("boom".into());
        let got = merged(Err(failed.clone()), Ok(vec![]), Ok(vec![agent("c")]));
        assert_eq!(got, Err(failed));
    }

    fn emitted(
        herdr: &'static [Status],
        kitty: &'static [Status],
        hooks: &'static [Status],
    ) -> &'static [Status] {
        let fake = |emitted| {
            Box::new(Fake {
                result: Ok(vec![]),
                emitted,
            })
        };
        MergedSource::new(fake(herdr), fake(kitty), fake(hooks)).emitted_statuses()
    }

    #[test]
    fn emitted_statuses_are_the_union() {
        let hooks = &[Status::Done, Status::Idle, Status::Working, Status::Blocked];
        assert_eq!(emitted(&Status::ALL, &[], hooks), Status::ALL);
    }

    #[test]
    fn emitted_statuses_follow_the_status_order() {
        assert_eq!(
            emitted(&[Status::Working], &[], &[Status::Done]),
            [Status::Done, Status::Working]
        );
    }

    #[test]
    fn emitted_statuses_include_what_only_kitty_emits() {
        assert_eq!(
            emitted(&[Status::Working], &[Status::Blocked], &[Status::Done]),
            [Status::Blocked, Status::Done, Status::Working]
        );
    }

    fn proc(name: &str, pid: Option<u32>) -> Agent {
        Agent {
            identities: pid.map(Identity::Pid).into_iter().collect(),
            ..agent(name)
        }
    }

    #[test]
    fn a_hook_session_with_kittys_pid_is_dropped() {
        let got = merged(
            Err(not_found()),
            Ok(vec![proc("k", Some(7))]),
            Ok(vec![proc("dup", Some(7))]),
        );
        assert_eq!(got, Ok(vec![proc("k", Some(7))]));
    }

    #[test]
    fn different_pids_or_a_missing_pid_keep_both() {
        let got = merged(
            Err(not_found()),
            Ok(vec![proc("k", Some(7)), proc("k2", None)]),
            Ok(vec![proc("h1", Some(8)), proc("h2", None)]),
        );
        assert_eq!(
            got,
            Ok(vec![
                proc("k", Some(7)),
                proc("k2", None),
                proc("h1", Some(8)),
                proc("h2", None)
            ])
        );
    }

    #[test]
    fn kitty_cards_sharing_a_pid_both_stay_but_a_hook_with_that_pid_goes() {
        let got = merged(
            Err(not_found()),
            Ok(vec![proc("root", Some(7)), proc("child", Some(7))]),
            Ok(vec![proc("dup", Some(7))]),
        );
        assert_eq!(got, Ok(vec![proc("root", Some(7)), proc("child", Some(7))]));
    }

    #[test]
    fn a_kitty_card_with_herdrs_pid_or_session_is_dropped() {
        let got = merged(
            Ok(vec![proc("h1", Some(1)), session("h2", Some("s"))]),
            Ok(vec![proc("k1", Some(1)), session("k2", Some("s"))]),
            Ok(vec![]),
        );
        assert_eq!(got, Ok(vec![proc("h1", Some(1)), session("h2", Some("s"))]));
    }

    #[test]
    fn without_herdr_kitty_cards_are_shown_even_with_hooks_empty() {
        let got = merged(Err(not_found()), Ok(vec![agent("k")]), Ok(vec![]));
        assert_eq!(got, Ok(vec![agent("k")]));
    }

    #[test]
    fn a_failing_kitty_adds_nothing_and_hooks_still_show() {
        let got = merged(
            Err(not_found()),
            Err(SourceError::Failed("boom".into())),
            Ok(vec![agent("c")]),
        );
        assert_eq!(got, Ok(vec![agent("c")]));
    }

    #[test]
    fn a_herdr_failure_is_reported_even_with_kitty_cards() {
        let failed = SourceError::Failed("boom".into());
        let got = merged(Err(failed.clone()), Ok(vec![agent("k")]), Ok(vec![]));
        assert_eq!(got, Err(failed));
    }
}
