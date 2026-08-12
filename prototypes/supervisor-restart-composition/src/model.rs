#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Approach {
    SupervisorOwned,
    OrdinaryRetry,
}

impl Approach {
    pub fn label(self) -> &'static str {
        match self {
            Self::SupervisorOwned => "supervisor-owned restart",
            Self::OrdinaryRetry => "ordinary Retry around each lifecycle",
        }
    }
}

#[derive(Debug)]
pub struct Member {
    pub name: &'static str,
    pub incarnation: u32,
    pub running: bool,
    pub unread_messages: usize,
    pub retry_attempts: u32,
}

impl Member {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            incarnation: 1,
            running: true,
            unread_messages: 2,
            retry_attempts: 0,
        }
    }
}

#[derive(Debug)]
pub struct Model {
    pub approach: Approach,
    pub supervisor_running: bool,
    pub shared_restart_attempts: u32,
    pub restart_limit: u32,
    pub members: [Member; 2],
    pub events: Vec<String>,
}

impl Model {
    pub fn new(approach: Approach) -> Self {
        Self {
            approach,
            supervisor_running: true,
            shared_restart_attempts: 0,
            restart_limit: 2,
            members: [Member::new("counter"), Member::new("cache")],
            events: vec!["supervisor started counter#1 and cache#1".into()],
        }
    }

    pub fn switch(&mut self) {
        let approach = match self.approach {
            Approach::SupervisorOwned => Approach::OrdinaryRetry,
            Approach::OrdinaryRetry => Approach::SupervisorOwned,
        };
        *self = Self::new(approach);
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.approach);
    }

    pub fn defect(&mut self, member_index: usize) {
        if !self.supervisor_running {
            self.events.push("ignored: supervisor is stopped".into());
            return;
        }

        let member = &mut self.members[member_index];
        if !member.running {
            self.events
                .push(format!("ignored: {} is stopped", member.name));
            return;
        }

        member.running = false;
        self.events.push(format!(
            "{}#{} defected; discarded {} unread messages",
            member.name, member.incarnation, member.unread_messages
        ));
        member.unread_messages = 0;

        match self.approach {
            Approach::SupervisorOwned => self.supervisor_restart(member_index),
            Approach::OrdinaryRetry => self.retry_restart(member_index),
        }
    }

    fn supervisor_restart(&mut self, member_index: usize) {
        self.shared_restart_attempts += 1;
        if self.shared_restart_attempts > self.restart_limit {
            self.supervisor_running = false;
            for member in &mut self.members {
                member.running = false;
                member.unread_messages = 0;
            }
            self.events.push(format!(
                "shared restart limit exceeded ({}/{}); supervisor stopped every member",
                self.shared_restart_attempts, self.restart_limit
            ));
            return;
        }

        let member = &mut self.members[member_index];
        member.incarnation += 1;
        member.running = true;
        member.unread_messages = 2;
        self.events.push(format!(
            "supervisor charged shared attempt {}/{} and started {}#{}",
            self.shared_restart_attempts, self.restart_limit, member.name, member.incarnation
        ));
    }

    fn retry_restart(&mut self, member_index: usize) {
        let member = &mut self.members[member_index];
        member.retry_attempts += 1;
        if member.retry_attempts > self.restart_limit {
            self.events.push(format!(
                "{} exhausted its private Retry budget; supervisor itself has no shared decision",
                member.name
            ));
            return;
        }

        member.incarnation += 1;
        member.running = true;
        member.unread_messages = 2;
        self.events.push(format!(
            "{} Retry charged private attempt {}/{} and started {}#{}",
            member.name,
            member.retry_attempts,
            self.restart_limit,
            member.name,
            member.incarnation
        ));
    }
}
