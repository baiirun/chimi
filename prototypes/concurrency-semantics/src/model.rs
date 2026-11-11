use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Runnable,
    Parked(String),
    Succeeded(String),
    Failed(String),
    Defected(String),
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded(_) | Self::Failed(_) | Self::Defected(_) | Self::Cancelled
        )
    }

    pub fn label(&self) -> String {
        match self {
            Self::Runnable => "runnable".into(),
            Self::Parked(reason) => format!("parked: {reason}"),
            Self::Succeeded(value) => format!("succeeded: {value}"),
            Self::Failed(error) => format!("failed: {error}"),
            Self::Defected(defect) => format!("defected: {defect}"),
            Self::Cancelled => "cancelled".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Task {
    pub owner: Option<String>,
    pub status: TaskStatus,
    pub observed: bool,
    pub last_result: Option<String>,
}

#[derive(Clone, Debug)]
pub struct WaitingSend {
    pub task: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChannelLifecycle {
    Open,
    Closed,
    Aborted,
}

impl ChannelLifecycle {
    pub fn label(&self, queue_empty: bool) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed if queue_empty => "closed/drained",
            Self::Closed => "closed/draining",
            Self::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub capacity: usize,
    pub queue: VecDeque<String>,
    pub lifecycle: ChannelLifecycle,
    pub waiting_senders: VecDeque<WaitingSend>,
    pub waiting_receiver: Option<String>,
}

#[derive(Clone, Debug)]
pub struct State {
    pub tasks: BTreeMap<String, Task>,
    pub channels: BTreeMap<String, Channel>,
    pub events: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    StartTask {
        owner: String,
        child: String,
    },
    Send {
        task: String,
        channel: String,
        value: String,
    },
    HostSend {
        channel: String,
        value: String,
    },
    Receive {
        task: String,
        channel: String,
    },
    Close {
        channel: String,
    },
    Abort {
        channel: String,
    },
    Cancel {
        task: String,
    },
    Succeed {
        task: String,
        value: String,
    },
    Fail {
        task: String,
        error: String,
    },
    Defect {
        task: String,
        cause: String,
    },
    Observe {
        owner: String,
        child: String,
    },
}

enum SendEffect {
    Fail(String),
    Direct(String),
    Buffered,
    Parked(usize),
}

enum ReceiveEffect {
    Fail(&'static str),
    Queued(String),
    Direct(WaitingSend),
    Drained,
    Parked,
}

impl State {
    pub fn new(tasks: &[(&str, Option<&str>)], channels: &[(&str, usize)]) -> Self {
        Self {
            tasks: tasks
                .iter()
                .map(|(name, owner)| {
                    (
                        (*name).into(),
                        Task {
                            owner: owner.map(str::to_owned),
                            status: TaskStatus::Runnable,
                            observed: false,
                            last_result: None,
                        },
                    )
                })
                .collect(),
            channels: channels
                .iter()
                .map(|(name, capacity)| {
                    (
                        (*name).into(),
                        Channel {
                            capacity: *capacity,
                            queue: VecDeque::new(),
                            lifecycle: ChannelLifecycle::Open,
                            waiting_senders: VecDeque::new(),
                            waiting_receiver: None,
                        },
                    )
                })
                .collect(),
            events: vec!["scenario initialized".into()],
        }
    }

    pub fn apply(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::StartTask { owner, child } => self.start_task(owner, child),
            Action::Send {
                task,
                channel,
                value,
            } => self.send(task, channel, value),
            Action::HostSend { channel, value } => self.host_send(channel, value),
            Action::Receive { task, channel } => self.receive(task, channel),
            Action::Close { channel } => self.close(channel),
            Action::Abort { channel } => self.abort(channel),
            Action::Cancel { task } => self.cancel_tree(task),
            Action::Succeed { task, value } => {
                self.terminate(task, TaskStatus::Succeeded(value.clone()))
            }
            Action::Fail { task, error } => self.terminate(task, TaskStatus::Failed(error.clone())),
            Action::Defect { task, cause } => self.defect(task, cause),
            Action::Observe { owner, child } => self.observe(owner, child),
        }
    }

    fn start_task(&mut self, owner: &str, child: &str) -> Result<(), String> {
        self.runnable_task(owner)?;
        if self.tasks.contains_key(child) {
            return Err(format!("task {child} already exists"));
        }
        self.tasks.insert(
            child.into(),
            Task {
                owner: Some(owner.into()),
                status: TaskStatus::Runnable,
                observed: false,
                last_result: None,
            },
        );
        self.events
            .push(format!("{owner} started structured child {child}"));
        Ok(())
    }

    fn send(&mut self, task: &str, channel_name: &str, value: &str) -> Result<(), String> {
        self.runnable_task(task)?;
        let effect = {
            let channel = self.channel_mut(channel_name)?;
            if channel.lifecycle == ChannelLifecycle::Aborted {
                SendEffect::Fail(format!("Channel.Aborted({value})"))
            } else if channel.lifecycle == ChannelLifecycle::Closed {
                SendEffect::Fail(format!("Channel.Closed({value})"))
            } else if let Some(receiver) = channel.waiting_receiver.take() {
                SendEffect::Direct(receiver)
            } else if channel.queue.len() < channel.capacity {
                channel.queue.push_back(value.into());
                SendEffect::Buffered
            } else {
                channel.waiting_senders.push_back(WaitingSend {
                    task: task.into(),
                    value: value.into(),
                });
                SendEffect::Parked(channel.capacity)
            }
        };

        match effect {
            SendEffect::Fail(error) => {
                self.terminate(task, TaskStatus::Failed(error.into()))?;
            }
            SendEffect::Direct(receiver) => {
                let receiver_task = self
                    .tasks
                    .get_mut(&receiver)
                    .expect("waiting receiver exists");
                receiver_task.status = TaskStatus::Runnable;
                receiver_task.last_result = Some(format!("Received({value})"));
                self.events.push(format!(
                    "{task} sent {value} directly to parked {receiver} through {channel_name}"
                ));
            }
            SendEffect::Buffered => self
                .events
                .push(format!("{task} buffered {value} in {channel_name}")),
            SendEffect::Parked(capacity) => {
                self.tasks.get_mut(task).expect("active task exists").status =
                    TaskStatus::Parked(format!("send {channel_name}"));
                self.events.push(format!(
                    "{task} parked because {channel_name} reached capacity {capacity}"
                ));
            }
        }
        Ok(())
    }

    fn host_send(&mut self, channel_name: &str, value: &str) -> Result<(), String> {
        let receiver = {
            let channel = self.channel_mut(channel_name)?;
            if channel.lifecycle != ChannelLifecycle::Open {
                return Err(format!(
                    "host cannot deliver to {} {channel_name}",
                    channel.lifecycle.label(channel.queue.is_empty())
                ));
            }
            channel
                .waiting_receiver
                .take()
                .ok_or_else(|| format!("no task is waiting on {channel_name}"))?
        };
        let receiver_task = self
            .tasks
            .get_mut(&receiver)
            .expect("waiting receiver exists");
        receiver_task.status = TaskStatus::Runnable;
        receiver_task.last_result = Some(format!("Received({value})"));
        self.events.push(format!(
            "host delivered {value} to parked {receiver} through {channel_name}"
        ));
        Ok(())
    }

    fn receive(&mut self, task: &str, channel_name: &str) -> Result<(), String> {
        self.runnable_task(task)?;
        let effect = {
            let channel = self.channel_mut(channel_name)?;
            if channel.lifecycle == ChannelLifecycle::Aborted {
                ReceiveEffect::Fail("Channel.Aborted")
            } else if let Some(value) = channel.queue.pop_front() {
                ReceiveEffect::Queued(value)
            } else if let Some(waiting) = channel.waiting_senders.pop_front() {
                ReceiveEffect::Direct(waiting)
            } else if channel.lifecycle == ChannelLifecycle::Closed {
                ReceiveEffect::Drained
            } else {
                if channel.waiting_receiver.is_some() {
                    return Err(format!(
                        "{channel_name} already has its single waiting receiver"
                    ));
                }
                channel.waiting_receiver = Some(task.into());
                ReceiveEffect::Parked
            }
        };

        match effect {
            ReceiveEffect::Fail(error) => {
                self.terminate(task, TaskStatus::Failed(error.into()))?;
            }
            ReceiveEffect::Queued(value) => {
                self.tasks
                    .get_mut(task)
                    .expect("active task exists")
                    .last_result = Some(format!("Received({value})"));
                self.events
                    .push(format!("{task} received {value} from {channel_name}"));
                self.admit_waiting_sender(channel_name);
            }
            ReceiveEffect::Direct(waiting) => {
                self.tasks
                    .get_mut(task)
                    .expect("active task exists")
                    .last_result = Some(format!("Received({})", waiting.value));
                self.tasks
                    .get_mut(&waiting.task)
                    .expect("waiting sender exists")
                    .status = TaskStatus::Runnable;
                self.events.push(format!(
                    "{task} received {} directly and woke {}",
                    waiting.value, waiting.task
                ));
            }
            ReceiveEffect::Drained => {
                self.tasks
                    .get_mut(task)
                    .expect("active task exists")
                    .last_result = Some("Drained".into());
                self.events
                    .push(format!("{task} observed drained {channel_name}"));
            }
            ReceiveEffect::Parked => {
                self.tasks.get_mut(task).expect("active task exists").status =
                    TaskStatus::Parked(format!("receive {channel_name}"));
                self.events.push(format!(
                    "{task} parked waiting to receive from {channel_name}"
                ));
            }
        }
        Ok(())
    }

    fn close(&mut self, channel_name: &str) -> Result<(), String> {
        let (waiting_senders, waiting_receiver, queue_empty) = {
            let channel = self.channel_mut(channel_name)?;
            match channel.lifecycle {
                ChannelLifecycle::Open => channel.lifecycle = ChannelLifecycle::Closed,
                ChannelLifecycle::Closed => {
                    self.events.push(format!(
                        "{channel_name} was already closed; no state changed"
                    ));
                    return Ok(());
                }
                ChannelLifecycle::Aborted => {
                    self.events.push(format!(
                        "{channel_name} was already aborted; close changed nothing"
                    ));
                    return Ok(());
                }
            }
            (
                std::mem::take(&mut channel.waiting_senders),
                channel.waiting_receiver.take(),
                channel.queue.is_empty(),
            )
        };
        for sender in waiting_senders {
            self.terminate(
                &sender.task,
                TaskStatus::Failed(format!("Channel.Closed({})", sender.value)),
            )?;
        }
        if queue_empty && let Some(receiver) = waiting_receiver {
            let task = self
                .tasks
                .get_mut(&receiver)
                .expect("waiting receiver exists");
            task.status = TaskStatus::Runnable;
            task.last_result = Some("Drained".into());
        }
        self.events.push(format!(
            "{channel_name} closed gracefully; accepted values remain"
        ));
        Ok(())
    }

    fn abort(&mut self, channel_name: &str) -> Result<(), String> {
        let (discarded, waiting_senders, waiting_receiver) = {
            let channel = self.channel_mut(channel_name)?;
            if channel.lifecycle == ChannelLifecycle::Aborted {
                self.events.push(format!(
                    "{channel_name} was already aborted; no state changed"
                ));
                return Ok(());
            }
            channel.lifecycle = ChannelLifecycle::Aborted;
            let discarded = channel.queue.len();
            channel.queue.clear();
            (
                discarded,
                std::mem::take(&mut channel.waiting_senders),
                channel.waiting_receiver.take(),
            )
        };
        for sender in waiting_senders {
            self.terminate(
                &sender.task,
                TaskStatus::Failed(format!("Channel.Aborted({})", sender.value)),
            )?;
        }
        if let Some(receiver) = waiting_receiver {
            self.terminate(&receiver, TaskStatus::Failed("Channel.Aborted".into()))?;
        }
        self.events.push(format!(
            "{channel_name} aborted destructively; discarded {discarded} buffered value(s)"
        ));
        Ok(())
    }

    fn admit_waiting_sender(&mut self, channel_name: &str) {
        let Some(channel) = self.channels.get_mut(channel_name) else {
            return;
        };
        if channel.lifecycle != ChannelLifecycle::Open || channel.queue.len() >= channel.capacity {
            return;
        }
        if let Some(waiting) = channel.waiting_senders.pop_front() {
            channel.queue.push_back(waiting.value.clone());
            self.tasks
                .get_mut(&waiting.task)
                .expect("waiting sender exists")
                .status = TaskStatus::Runnable;
            self.events.push(format!(
                "{} resumed; {} entered {channel_name}",
                waiting.task, waiting.value
            ));
        }
    }

    fn observe(&mut self, owner: &str, child: &str) -> Result<(), String> {
        self.runnable_task(owner)?;
        let child_task = self
            .tasks
            .get(child)
            .ok_or_else(|| format!("unknown child {child}"))?;
        if child_task.owner.as_deref() != Some(owner) {
            return Err(format!("{owner} does not own {child}"));
        }
        let outcome = child_task.status.clone();
        if !outcome.is_terminal() {
            return Err(format!("{child} has no terminal result"));
        }
        self.tasks.get_mut(child).expect("child exists").observed = true;
        self.events
            .push(format!("{owner} observed {child}: {}", outcome.label()));
        match outcome {
            TaskStatus::Succeeded(value) => {
                self.tasks.get_mut(owner).expect("owner exists").last_result =
                    Some(format!("Success({value})"));
            }
            TaskStatus::Failed(error) => {
                self.tasks.get_mut(owner).expect("owner exists").status = TaskStatus::Failed(error);
                self.cancel_active_children(owner, Some(child));
            }
            TaskStatus::Defected(cause) => {
                self.tasks.get_mut(owner).expect("owner exists").status =
                    TaskStatus::Defected(cause);
                self.cancel_active_children(owner, Some(child));
            }
            TaskStatus::Cancelled => {
                self.tasks.get_mut(owner).expect("owner exists").status = TaskStatus::Cancelled;
                self.cancel_active_children(owner, Some(child));
            }
            TaskStatus::Runnable | TaskStatus::Parked(_) => unreachable!(),
        }
        Ok(())
    }

    fn terminate(&mut self, task: &str, outcome: TaskStatus) -> Result<(), String> {
        self.live_task(task)?;
        self.remove_waiter(task);
        self.tasks.get_mut(task).expect("active task exists").status = outcome.clone();
        self.events
            .push(format!("{task} became terminal: {}", outcome.label()));
        self.cancel_active_children(task, None);
        Ok(())
    }

    fn defect(&mut self, task: &str, cause: &str) -> Result<(), String> {
        self.terminate(task, TaskStatus::Defected(cause.into()))?;
        let mut failed_child = task.to_owned();
        while let Some(owner) = self
            .tasks
            .get(&failed_child)
            .and_then(|task| task.owner.clone())
        {
            let owner_status = self
                .tasks
                .get(&owner)
                .expect("task owner exists")
                .status
                .clone();
            if owner_status.is_terminal() {
                break;
            }
            self.remove_waiter(&owner);
            self.tasks
                .get_mut(&owner)
                .expect("task owner exists")
                .status = TaskStatus::Defected(cause.into());
            self.cancel_active_children(&owner, Some(&failed_child));
            self.events.push(format!(
                "{owner} defected because structured child {failed_child} defected: {cause}"
            ));
            failed_child = owner;
        }
        Ok(())
    }

    fn cancel_tree(&mut self, task: &str) -> Result<(), String> {
        let status = self
            .tasks
            .get(task)
            .ok_or_else(|| format!("unknown task {task}"))?
            .status
            .clone();
        if status.is_terminal() {
            return Ok(());
        }
        self.remove_waiter(task);
        self.tasks.get_mut(task).expect("task exists").status = TaskStatus::Cancelled;
        self.events.push(format!("{task} cancelled"));
        self.cancel_active_children(task, None);
        Ok(())
    }

    fn cancel_active_children(&mut self, owner: &str, except: Option<&str>) {
        let children: Vec<String> = self
            .tasks
            .iter()
            .filter(|(name, task)| {
                task.owner.as_deref() == Some(owner)
                    && !task.status.is_terminal()
                    && except != Some(name.as_str())
            })
            .map(|(name, _)| name.clone())
            .collect();
        for child in children {
            let _ = self.cancel_tree(&child);
        }
    }

    fn remove_waiter(&mut self, task: &str) {
        for channel in self.channels.values_mut() {
            channel
                .waiting_senders
                .retain(|waiting| waiting.task != task);
            if channel.waiting_receiver.as_deref() == Some(task) {
                channel.waiting_receiver = None;
            }
        }
    }

    fn runnable_task(&self, name: &str) -> Result<(), String> {
        let task = self
            .tasks
            .get(name)
            .ok_or_else(|| format!("unknown task {name}"))?;
        match task.status {
            TaskStatus::Runnable => Ok(()),
            TaskStatus::Parked(_) => Err(format!("task {name} is parked and cannot run")),
            _ => Err(format!("task {name} is already terminal")),
        }
    }

    fn live_task(&self, name: &str) -> Result<(), String> {
        let task = self
            .tasks
            .get(name)
            .ok_or_else(|| format!("unknown task {name}"))?;
        if task.status.is_terminal() {
            Err(format!("task {name} is already terminal"))
        } else {
            Ok(())
        }
    }

    fn channel_mut(&mut self, name: &str) -> Result<&mut Channel, String> {
        self.channels
            .get_mut(name)
            .ok_or_else(|| format!("unknown channel {name}"))
    }
}
