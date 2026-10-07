use crate::model::{Agent, Status};

/// The one order, most urgent first, shared by the menu bar and the list so the icon on
/// show is always the head of the list. Always holds each of the five exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priority([Status; Status::COUNT]);

impl Default for Priority {
    fn default() -> Self {
        Priority(Status::ALL)
    }
}

impl Priority {
    pub fn order(&self) -> &[Status; Status::COUNT] {
        &self.0
    }

    /// Comma-separated names. `None` unless it names each of the five exactly once.
    pub fn parse(s: &str) -> Option<Priority> {
        let parsed: Vec<Status> = s
            .split(',')
            .map(|p| Status::parse(p.trim()))
            .collect::<Option<_>>()?;
        let order: [Status; Status::COUNT] = parsed.try_into().ok()?;
        Status::ALL
            .iter()
            .all(|st| order.contains(st))
            .then_some(Priority(order))
    }

    pub fn serialize(&self) -> String {
        self.0.map(Status::as_str).join(",")
    }

    fn rank(&self, status: Status) -> usize {
        self.0
            .iter()
            .position(|&s| s == status)
            .expect("priority holds every status")
    }

    /// No-op for the first entry.
    pub fn move_up(&mut self, status: Status) {
        let i = self.rank(status);
        if i > 0 {
            self.0.swap(i, i - 1);
        }
    }

    /// No-op for the last entry.
    pub fn move_down(&mut self, status: Status) {
        let i = self.rank(status);
        if i + 1 < self.0.len() {
            self.0.swap(i, i + 1);
        }
    }

    /// Stable, so agents of one status keep the source's order.
    pub fn sort(&self, agents: &mut [Agent]) {
        agents.sort_by_key(|a| self.rank(a.status));
    }

    /// Per-status counts in priority order, statuses with no agent left out.
    pub fn counts(&self, agents: &[Agent]) -> Vec<(Status, usize)> {
        self.0
            .iter()
            .map(|&st| (st, agents.iter().filter(|a| a.status == st).count()))
            .filter(|&(_, n)| n > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(status: Status, name: &str) -> Agent {
        Agent {
            status,
            name: name.into(),
            workspace: String::new(),
            title: String::new(),
            focused: false,
        }
    }

    #[test]
    fn the_default_order_is_blocked_done_idle_working_unknown() {
        assert_eq!(
            Priority::default().serialize(),
            "blocked,done,idle,working,unknown"
        );
    }

    #[test]
    fn a_full_permutation_round_trips() {
        let p = Priority::parse("idle, working,blocked,unknown,done").unwrap();
        assert_eq!(p.serialize(), "idle,working,blocked,unknown,done");
        assert_eq!(Priority::parse(&p.serialize()), Some(p));
    }

    #[test]
    fn anything_but_each_status_once_is_rejected() {
        for s in [
            "",
            "blocked,done,idle,working",
            "blocked,done,idle,working,unknown,done",
            "blocked,blocked,idle,working,unknown",
            "blocked,done,idle,working,waiting",
        ] {
            assert_eq!(Priority::parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn moving_swaps_with_the_neighbour_and_stops_at_the_ends() {
        let mut p = Priority::default();
        p.move_up(Status::Idle);
        assert_eq!(p.serialize(), "blocked,idle,done,working,unknown");
        p.move_down(Status::Blocked);
        assert_eq!(p.serialize(), "idle,blocked,done,working,unknown");
        p.move_up(Status::Idle);
        p.move_down(Status::Unknown);
        assert_eq!(p.serialize(), "idle,blocked,done,working,unknown");
    }

    #[test]
    fn sort_follows_priority_and_keeps_source_order_within_a_status() {
        let mut agents = vec![
            agent(Status::Working, "w1"),
            agent(Status::Idle, "i1"),
            agent(Status::Blocked, "b1"),
            agent(Status::Idle, "i2"),
        ];
        Priority::default().sort(&mut agents);
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["b1", "i1", "i2", "w1"]);
    }

    #[test]
    fn counts_are_in_priority_order_without_zeros() {
        let agents = vec![
            agent(Status::Working, "a"),
            agent(Status::Idle, "b"),
            agent(Status::Idle, "c"),
            agent(Status::Blocked, "d"),
        ];
        assert_eq!(
            Priority::default().counts(&agents),
            vec![
                (Status::Blocked, 1),
                (Status::Idle, 2),
                (Status::Working, 1)
            ]
        );
        assert!(Priority::default().counts(&[]).is_empty());
    }
}
