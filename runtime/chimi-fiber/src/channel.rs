use crate::scheduler::TaskId;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChannelId(u64);

impl ChannelId {
    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sender {
    channel: ChannelId,
}

impl Sender {
    pub fn channel(self) -> ChannelId {
        self.channel
    }

    pub(crate) const fn from_raw(channel: u64) -> Self {
        Self {
            channel: ChannelId::from_raw(channel),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Receiver {
    channel: ChannelId,
    owner: TaskId,
    token: u64,
}

impl Receiver {
    pub fn channel(self) -> ChannelId {
        self.channel
    }

    pub fn owner(self) -> TaskId {
        self.owner
    }

    pub(crate) fn token(self) -> u64 {
        self.token
    }

    pub(crate) const fn from_raw(channel: u64, owner: u64, token: u64) -> Self {
        Self {
            channel: ChannelId::from_raw(channel),
            owner: TaskId::from_raw(owner),
            token,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Completion {
    channel: ChannelId,
    owner: TaskId,
    token: u64,
}

impl Completion {
    pub fn channel(self) -> ChannelId {
        self.channel
    }

    pub fn owner(self) -> TaskId {
        self.owner
    }

    pub(crate) fn token(self) -> u64 {
        self.token
    }

    pub(crate) const fn from_raw(channel: u64, owner: u64, token: u64) -> Self {
        Self {
            channel: ChannelId::from_raw(channel),
            owner: TaskId::from_raw(owner),
            token,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelCapabilities {
    pub sender: Sender,
    pub receiver: Receiver,
    pub completion: Completion,
}

/// Which exclusive capability of a channel a value holds.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CapabilityKind {
    Receiver,
    Completion,
}

/// A Receiver or Completion inside a value sent through a channel, with the
/// token of the sender's copy. Accepting the send puts the capability in
/// flight with the next token, which the host writes into the message's copy:
/// the sender's own copies are stale from then on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Carried {
    pub channel: ChannelId,
    pub kind: CapabilityKind,
    pub token: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelMetrics {
    pub capacity: usize,
    pub queue_depth: usize,
    pub waiting_senders: usize,
    pub waiting_receiver: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendResult {
    Pending,
    Accepted,
    Closed(usize),
    Aborted(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveResult {
    Pending,
    Received(usize),
    Drained,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    UnknownChannel(ChannelId),
    WrongReceiverOwner {
        channel: ChannelId,
        expected: Option<TaskId>,
        actual: TaskId,
    },
    WrongCompletionOwner {
        channel: ChannelId,
        expected: Option<TaskId>,
        actual: TaskId,
    },
    OverlappingOperation(TaskId),
    CapabilityHasPendingOperation(TaskId),
    ChannelStillLive(ChannelId),
    /// The capability travels in a message no task has received yet, so the
    /// copy used is stale.
    CapabilityInFlight {
        channel: ChannelId,
        kind: CapabilityKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ChannelEventKind {
    Created,
    SendAccepted,
    SendParked,
    SendRejectedClosed,
    SendRejectedAborted,
    SendCancelled,
    ReceiveDelivered,
    ReceiveParked,
    ReceiveCancelled,
    Closed,
    Drained,
    Aborted,
    ReceiverTransferred,
    CompletionTransferred,
    /// The host released the channel; its id is no longer valid.
    Released,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Observation {
    pub channel: ChannelId,
    pub task: Option<TaskId>,
    pub kind: ChannelEventKind,
    pub cause: Option<TaskId>,
    pub capacity: usize,
    pub queue_depth: usize,
    pub waiter_depth: usize,
    pub discard_count: usize,
}

struct ChannelObservation {
    channel: ChannelId,
    task: Option<TaskId>,
    kind: ChannelEventKind,
    cause: Option<TaskId>,
    discard_count: usize,
}

#[derive(Default)]
pub(crate) struct Transition {
    pub wakes: Vec<TaskId>,
    pub observations: Vec<Observation>,
    /// Accepted values the registry gave up without delivering them to a
    /// receiver or handing them back to their sender. The scheduler
    /// releases each one.
    pub discarded: Vec<usize>,
    /// Values given to a receiver, and values handed back to their sender:
    /// `settle_parcels` moves the capabilities each one carries.
    delivered: Vec<(usize, TaskId)>,
    returned: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    Open,
    Closed,
    Aborted,
}

struct WaitingSend {
    task: TaskId,
    value: usize,
}

/// The exclusive capabilities one sent value carries, from its acceptance
/// until it is delivered, handed back, or discarded. `carried` keeps the
/// sender's tokens, so a hand-back restores the sender's copies.
struct Parcel {
    sender: TaskId,
    carried: Vec<Carried>,
}

struct Channel {
    capacity: usize,
    queue: VecDeque<usize>,
    lifecycle: Lifecycle,
    waiting_senders: VecDeque<WaitingSend>,
    waiting_receiver: Option<TaskId>,
    receiver_owner: Option<TaskId>,
    receiver_token: u64,
    completion_owner: Option<TaskId>,
    completion_token: u64,
    /// The sent value whose parcel holds the receiver or the completion. A
    /// capability in flight has no owner.
    receiver_parcel: Option<usize>,
    completion_parcel: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Sending { channel: ChannelId },
    SendReady(SendResult),
    Receiving { channel: ChannelId },
    ReceiveReady(ReceiveResult),
}

pub(crate) struct ChannelRegistry {
    channels: BTreeMap<ChannelId, Channel>,
    owned_channels: BTreeMap<TaskId, BTreeSet<ChannelId>>,
    operations: BTreeMap<TaskId, Operation>,
    /// Keyed by the sent value, which is unique while the registry holds it.
    parcels: BTreeMap<usize, Parcel>,
    next_channel: u64,
    observe_boundaries: bool,
    observe_full: bool,
}

impl ChannelRegistry {
    pub fn new(observe_boundaries: bool, observe_full: bool) -> Self {
        Self {
            channels: BTreeMap::new(),
            owned_channels: BTreeMap::new(),
            operations: BTreeMap::new(),
            parcels: BTreeMap::new(),
            next_channel: 1,
            observe_boundaries,
            observe_full,
        }
    }

    pub fn create(&mut self, owner: TaskId, capacity: usize) -> (ChannelCapabilities, Transition) {
        let id = ChannelId(self.next_channel);
        self.next_channel = self
            .next_channel
            .checked_add(1)
            .expect("channel identity space exhausted");
        self.channels.insert(
            id,
            Channel {
                capacity,
                queue: VecDeque::new(),
                lifecycle: Lifecycle::Open,
                waiting_senders: VecDeque::new(),
                waiting_receiver: None,
                receiver_owner: Some(owner),
                receiver_token: 1,
                completion_owner: Some(owner),
                completion_token: 1,
                receiver_parcel: None,
                completion_parcel: None,
            },
        );
        self.owned_channels.entry(owner).or_default().insert(id);
        let capabilities = ChannelCapabilities {
            sender: Sender { channel: id },
            receiver: Receiver {
                channel: id,
                owner,
                token: 1,
            },
            completion: Completion {
                channel: id,
                owner,
                token: 1,
            },
        };
        let mut transition = Transition::default();
        record(
            &mut transition,
            self.observe_boundaries,
            Observation {
                channel: id,
                task: Some(owner),
                kind: ChannelEventKind::Created,
                cause: None,
                capacity,
                queue_depth: 0,
                waiter_depth: 0,
                discard_count: 0,
            },
        );
        (capabilities, transition)
    }

    /// Sends `value`, which holds the `carried` capabilities of `task`. Once
    /// accepted or parked, each is in flight with its next token until the
    /// value reaches a receiver, who then owns it; a refused value hands
    /// them back with their old tokens, and a discarded one aborts their
    /// channels, as its owner's exit would have. Nothing changes when any
    /// carried capability is not `task`'s.
    pub fn send_carrying(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
        carried: &[Carried],
    ) -> Result<(SendResult, Transition), ChannelError> {
        self.begin_operation(task)?;
        if !self.channels.contains_key(&sender.channel) {
            return Err(ChannelError::UnknownChannel(sender.channel));
        }
        let carried = self.validate_carried(task, carried)?;
        if !carried.is_empty() {
            self.dispatch_parcel(task, value, carried);
        }
        let mut transition = self.send_value(task, sender, value)?;
        self.settle_parcels(&mut transition.1);
        Ok(transition)
    }

    /// Checks that every capability in `carried` is `task`'s current copy,
    /// before a message that holds them leaves `task` outside a channel (a
    /// process mailbox, or a process start's argument). Changes nothing.
    pub fn check_carried(&self, task: TaskId, carried: &[Carried]) -> Result<(), ChannelError> {
        self.validate_carried(task, carried).map(|_| ())
    }

    /// Puts `carried`, already checked by `check_carried`, in flight in
    /// `value`'s parcel once something other than a channel has accepted
    /// `value` from `task`. `deliver_parcel` or `discard_parcel` settles
    /// it.
    pub fn dispatch_carried(&mut self, task: TaskId, value: usize, carried: &[Carried]) {
        let carried = self
            .validate_carried(task, carried)
            .expect("check_carried accepted these capabilities");
        if !carried.is_empty() {
            self.dispatch_parcel(task, value, carried);
        }
    }

    /// Gives the capabilities `value` carries to `receiver`.
    pub fn deliver_parcel(&mut self, value: usize, receiver: TaskId) {
        let mut transition = Transition::default();
        transition.delivered.push((value, receiver));
        self.settle_parcels(&mut transition);
    }

    /// Ends the capabilities `value` carries, which was dropped unread, as
    /// their owner's exit would: open channels abort.
    pub fn discard_parcel(&mut self, value: usize) -> Transition {
        let mut transition = Transition::default();
        transition.discarded.push(value);
        self.settle_parcels(&mut transition);
        // `value` is not a channel's, so its owner releases it; values the
        // aborts discarded follow it and are the channels' to release.
        transition.discarded.remove(0);
        transition
    }

    fn send_value(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
    ) -> Result<(SendResult, Transition), ChannelError> {
        let observe_boundaries = self.observe_boundaries;
        let observe_full = self.observe_full;
        let channel = self
            .channels
            .get_mut(&sender.channel)
            .ok_or(ChannelError::UnknownChannel(sender.channel))?;
        let mut transition = Transition::default();

        let result = match channel.lifecycle {
            Lifecycle::Closed => SendResult::Closed(value),
            Lifecycle::Aborted => SendResult::Aborted(value),
            Lifecycle::Open => {
                if let Some(receiver) = channel.waiting_receiver.take() {
                    self.operations.insert(
                        receiver,
                        Operation::ReceiveReady(ReceiveResult::Received(value)),
                    );
                    transition.delivered.push((value, receiver));
                    transition.wakes.push(receiver);
                    observe_channel(
                        &mut transition,
                        observe_full,
                        channel,
                        ChannelObservation {
                            channel: sender.channel,
                            task: Some(receiver),
                            kind: ChannelEventKind::ReceiveDelivered,
                            cause: Some(task),
                            discard_count: 0,
                        },
                    );
                    SendResult::Accepted
                } else if channel.queue.len() < channel.capacity {
                    debug_assert!(channel.waiting_senders.is_empty());
                    channel.queue.push_back(value);
                    SendResult::Accepted
                } else {
                    channel
                        .waiting_senders
                        .push_back(WaitingSend { task, value });
                    self.operations.insert(
                        task,
                        Operation::Sending {
                            channel: sender.channel,
                        },
                    );
                    observe_channel(
                        &mut transition,
                        observe_full,
                        channel,
                        ChannelObservation {
                            channel: sender.channel,
                            task: Some(task),
                            kind: ChannelEventKind::SendParked,
                            cause: None,
                            discard_count: 0,
                        },
                    );
                    channel.assert_invariants();
                    return Ok((SendResult::Pending, transition));
                }
            }
        };

        self.operations.insert(task, Operation::SendReady(result));
        transition.wakes.push(task);
        if let SendResult::Closed(value) | SendResult::Aborted(value) = result {
            transition.returned.push(value);
        }
        let (kind, enabled) = match result {
            SendResult::Accepted => (ChannelEventKind::SendAccepted, observe_full),
            SendResult::Closed(_) => (ChannelEventKind::SendRejectedClosed, observe_boundaries),
            SendResult::Aborted(_) => (ChannelEventKind::SendRejectedAborted, observe_boundaries),
            SendResult::Pending => unreachable!("pending returned above"),
        };
        observe_channel(
            &mut transition,
            enabled,
            channel,
            ChannelObservation {
                channel: sender.channel,
                task: Some(task),
                kind,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
        Ok((result, transition))
    }

    pub fn receive(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<(ReceiveResult, Transition), ChannelError> {
        let mut received = self.receive_value(task, receiver)?;
        self.settle_parcels(&mut received.1);
        Ok(received)
    }

    fn receive_value(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<(ReceiveResult, Transition), ChannelError> {
        self.begin_operation(task)?;
        let observe_boundaries = self.observe_boundaries;
        let observe_full = self.observe_full;
        let channel = self
            .channels
            .get_mut(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(channel, receiver, task)?;
        let mut transition = Transition::default();

        let result = if channel.lifecycle == Lifecycle::Aborted {
            ReceiveResult::Aborted
        } else if let Some(value) = channel.queue.pop_front() {
            if let Some(waiter) = channel.waiting_senders.pop_front() {
                debug_assert!(channel.capacity > 0);
                channel.queue.push_back(waiter.value);
                self.operations
                    .insert(waiter.task, Operation::SendReady(SendResult::Accepted));
                transition.wakes.push(waiter.task);
                observe_channel(
                    &mut transition,
                    observe_full,
                    channel,
                    ChannelObservation {
                        channel: receiver.channel,
                        task: Some(waiter.task),
                        kind: ChannelEventKind::SendAccepted,
                        cause: Some(task),
                        discard_count: 0,
                    },
                );
            }
            transition.delivered.push((value, task));
            ReceiveResult::Received(value)
        } else if let Some(waiter) = channel.waiting_senders.pop_front() {
            debug_assert_eq!(channel.capacity, 0);
            self.operations
                .insert(waiter.task, Operation::SendReady(SendResult::Accepted));
            transition.wakes.push(waiter.task);
            observe_channel(
                &mut transition,
                observe_full,
                channel,
                ChannelObservation {
                    channel: receiver.channel,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendAccepted,
                    cause: Some(task),
                    discard_count: 0,
                },
            );
            transition.delivered.push((waiter.value, task));
            ReceiveResult::Received(waiter.value)
        } else if channel.lifecycle == Lifecycle::Closed {
            ReceiveResult::Drained
        } else {
            debug_assert!(channel.waiting_receiver.is_none());
            channel.waiting_receiver = Some(task);
            self.operations.insert(
                task,
                Operation::Receiving {
                    channel: receiver.channel,
                },
            );
            observe_channel(
                &mut transition,
                observe_full,
                channel,
                ChannelObservation {
                    channel: receiver.channel,
                    task: Some(task),
                    kind: ChannelEventKind::ReceiveParked,
                    cause: None,
                    discard_count: 0,
                },
            );
            channel.assert_invariants();
            return Ok((ReceiveResult::Pending, transition));
        };

        self.operations
            .insert(task, Operation::ReceiveReady(result));
        transition.wakes.push(task);
        let (kind, enabled) = match result {
            ReceiveResult::Received(_) => (ChannelEventKind::ReceiveDelivered, observe_full),
            ReceiveResult::Drained => (ChannelEventKind::Drained, observe_boundaries),
            ReceiveResult::Aborted => (ChannelEventKind::Aborted, observe_boundaries),
            ReceiveResult::Pending => unreachable!("pending returned above"),
        };
        observe_channel(
            &mut transition,
            enabled,
            channel,
            ChannelObservation {
                channel: receiver.channel,
                task: Some(task),
                kind,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
        Ok((result, transition))
    }

    pub fn close(
        &mut self,
        task: TaskId,
        completion: Completion,
    ) -> Result<Transition, ChannelError> {
        self.begin_operation(task)?;
        let observe_boundaries = self.observe_boundaries;
        let channel = self
            .channels
            .get_mut(&completion.channel)
            .ok_or(ChannelError::UnknownChannel(completion.channel))?;
        validate_completion(channel, completion, task)?;
        let mut transition = Transition::default();
        if channel.lifecycle != Lifecycle::Open {
            return Ok(transition);
        }
        channel.lifecycle = Lifecycle::Closed;
        while let Some(waiter) = channel.waiting_senders.pop_front() {
            self.operations.insert(
                waiter.task,
                Operation::SendReady(SendResult::Closed(waiter.value)),
            );
            transition.returned.push(waiter.value);
            transition.wakes.push(waiter.task);
            observe_channel(
                &mut transition,
                observe_boundaries,
                channel,
                ChannelObservation {
                    channel: completion.channel,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendRejectedClosed,
                    cause: Some(task),
                    discard_count: 0,
                },
            );
        }
        if channel.queue.is_empty()
            && let Some(receiver) = channel.waiting_receiver.take()
        {
            self.operations
                .insert(receiver, Operation::ReceiveReady(ReceiveResult::Drained));
            transition.wakes.push(receiver);
            observe_channel(
                &mut transition,
                observe_boundaries,
                channel,
                ChannelObservation {
                    channel: completion.channel,
                    task: Some(receiver),
                    kind: ChannelEventKind::Drained,
                    cause: Some(task),
                    discard_count: 0,
                },
            );
        }
        observe_channel(
            &mut transition,
            observe_boundaries,
            channel,
            ChannelObservation {
                channel: completion.channel,
                task: Some(task),
                kind: ChannelEventKind::Closed,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    pub fn abort(&mut self, task: TaskId, receiver: Receiver) -> Result<Transition, ChannelError> {
        self.begin_operation(task)?;
        let channel = self
            .channels
            .get(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(channel, receiver, task)?;
        let mut transition =
            self.abort_channel(receiver.channel, Some(task), ChannelEventKind::Aborted);
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    /// Ends a channel the host no longer references: it is aborted, which
    /// discards its queue and answers its parked peers, and then forgotten,
    /// so any later use of its id is an unknown channel.
    pub fn release_unreferenced(
        &mut self,
        task: TaskId,
        id: ChannelId,
    ) -> Result<Transition, ChannelError> {
        self.begin_operation(task)?;
        let owners = self
            .channels
            .get(&id)
            .map(|channel| [channel.receiver_owner, channel.completion_owner])
            .ok_or(ChannelError::UnknownChannel(id))?;
        let mut transition = self.abort_channel(id, Some(task), ChannelEventKind::Released);
        self.channels.remove(&id);
        for owner in owners.into_iter().flatten() {
            self.refresh_owner_index(id, owner);
        }
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    pub fn transfer_receiver(
        &mut self,
        task: TaskId,
        receiver: Receiver,
        new_owner: TaskId,
    ) -> Result<(Receiver, Transition), ChannelError> {
        self.begin_operation(task)?;
        let observe_boundaries = self.observe_boundaries;
        let channel = self
            .channels
            .get_mut(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(channel, receiver, task)?;
        if channel.waiting_receiver.is_some() {
            return Err(ChannelError::CapabilityHasPendingOperation(task));
        }
        channel.receiver_owner = Some(new_owner);
        channel.receiver_token = channel
            .receiver_token
            .checked_add(1)
            .expect("receiver token exhausted");
        let transferred = Receiver {
            channel: receiver.channel,
            owner: new_owner,
            token: channel.receiver_token,
        };
        let mut transition = Transition::default();
        observe_channel(
            &mut transition,
            observe_boundaries,
            channel,
            ChannelObservation {
                channel: receiver.channel,
                task: Some(task),
                kind: ChannelEventKind::ReceiverTransferred,
                cause: Some(new_owner),
                discard_count: 0,
            },
        );
        self.refresh_owner_index(receiver.channel, task);
        self.refresh_owner_index(receiver.channel, new_owner);
        Ok((transferred, transition))
    }

    pub fn transfer_completion(
        &mut self,
        task: TaskId,
        completion: Completion,
        new_owner: TaskId,
    ) -> Result<(Completion, Transition), ChannelError> {
        self.begin_operation(task)?;
        let observe_boundaries = self.observe_boundaries;
        let channel = self
            .channels
            .get_mut(&completion.channel)
            .ok_or(ChannelError::UnknownChannel(completion.channel))?;
        validate_completion(channel, completion, task)?;
        channel.completion_owner = Some(new_owner);
        channel.completion_token = channel
            .completion_token
            .checked_add(1)
            .expect("completion token exhausted");
        let transferred = Completion {
            channel: completion.channel,
            owner: new_owner,
            token: channel.completion_token,
        };
        let mut transition = Transition::default();
        observe_channel(
            &mut transition,
            observe_boundaries,
            channel,
            ChannelObservation {
                channel: completion.channel,
                task: Some(task),
                kind: ChannelEventKind::CompletionTransferred,
                cause: Some(new_owner),
                discard_count: 0,
            },
        );
        self.refresh_owner_index(completion.channel, task);
        self.refresh_owner_index(completion.channel, new_owner);
        Ok((transferred, transition))
    }

    /// True while the task has a send or receive registered, pending or with
    /// a ready result it has not collected.
    pub fn has_operation(&self, task: TaskId) -> bool {
        self.operations.contains_key(&task)
    }

    pub fn take_send_result(&mut self, task: TaskId) -> Option<SendResult> {
        match self.operations.get(&task).copied() {
            Some(Operation::SendReady(result)) => {
                self.operations.remove(&task);
                Some(result)
            }
            _ => None,
        }
    }

    pub fn take_receive_result(&mut self, task: TaskId) -> Option<ReceiveResult> {
        match self.operations.get(&task).copied() {
            Some(Operation::ReceiveReady(result)) => {
                self.operations.remove(&task);
                Some(result)
            }
            _ => None,
        }
    }

    pub fn metrics(&self, id: ChannelId) -> Result<ChannelMetrics, ChannelError> {
        let channel = self
            .channels
            .get(&id)
            .ok_or(ChannelError::UnknownChannel(id))?;
        Ok(ChannelMetrics {
            capacity: channel.capacity,
            queue_depth: channel.queue.len(),
            waiting_senders: channel.waiting_senders.len(),
            waiting_receiver: channel.waiting_receiver.is_some(),
        })
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn release(&mut self, id: ChannelId) -> Result<(), ChannelError> {
        let channel = self
            .channels
            .get(&id)
            .ok_or(ChannelError::UnknownChannel(id))?;
        let operation_is_live = self.operations.values().any(|operation| {
            matches!(
                operation,
                Operation::Sending { channel } | Operation::Receiving { channel }
                    if *channel == id
            )
        });
        if channel.lifecycle == Lifecycle::Open
            || channel.receiver_owner.is_some()
            || channel.completion_owner.is_some()
            || channel.receiver_parcel.is_some()
            || channel.completion_parcel.is_some()
            || !channel.queue.is_empty()
            || !channel.waiting_senders.is_empty()
            || channel.waiting_receiver.is_some()
            || operation_is_live
        {
            return Err(ChannelError::ChannelStillLive(id));
        }
        debug_assert!(
            self.owned_channels
                .values()
                .all(|channels| !channels.contains(&id))
        );
        self.channels.remove(&id);
        Ok(())
    }

    /// Gives up every value still queued, parked with a sender, or waiting
    /// in a ready result, for a scheduler that is being dropped.
    pub fn drain_held_values(&mut self) -> Vec<usize> {
        let mut values = Vec::new();
        for channel in self.channels.values_mut() {
            values.extend(channel.queue.drain(..));
            values.extend(channel.waiting_senders.drain(..).map(|waiter| waiter.value));
        }
        for (_, operation) in std::mem::take(&mut self.operations) {
            if let Some(value) = operation.held_value() {
                values.push(value);
            }
        }
        values
    }

    pub fn cleanup_task(&mut self, task: TaskId) -> Transition {
        let mut transition = self.cancel_operation(task);
        let channels = self.owned_channels.remove(&task).unwrap_or_default();
        for id in channels {
            let should_abort = self.channels.get(&id).is_some_and(|channel| {
                (channel.receiver_owner == Some(task)
                    && (channel.lifecycle == Lifecycle::Open
                        || (channel.lifecycle == Lifecycle::Closed && !channel.queue.is_empty())))
                    || channel.completion_owner == Some(task)
                        && channel.lifecycle == Lifecycle::Open
            });
            if should_abort {
                transition.merge(self.abort_channel(id, Some(task), ChannelEventKind::Aborted));
            }
            let channel = self.channels.get_mut(&id).expect("owned channel");
            if channel.receiver_owner == Some(task) {
                channel.receiver_owner = None;
            }
            if channel.completion_owner == Some(task) {
                channel.completion_owner = None;
            }
        }
        self.settle_parcels(&mut transition);
        transition
    }

    /// Withdraws the task's pending send or receive, leaving the channel as
    /// if the operation had never been issued. A ready result is dropped.
    pub fn cancel_pending(&mut self, task: TaskId) -> Transition {
        let mut transition = self.cancel_operation(task);
        self.settle_parcels(&mut transition);
        transition
    }

    /// The distinct capabilities `carried` names, once every one is `task`'s
    /// current copy.
    fn validate_carried(
        &self,
        task: TaskId,
        carried: &[Carried],
    ) -> Result<Vec<Carried>, ChannelError> {
        let mut distinct: Vec<Carried> = Vec::with_capacity(carried.len());
        for item in carried {
            let channel = self
                .channels
                .get(&item.channel)
                .ok_or(ChannelError::UnknownChannel(item.channel))?;
            match item.kind {
                CapabilityKind::Receiver => validate_receiver(
                    channel,
                    Receiver {
                        channel: item.channel,
                        owner: task,
                        token: item.token,
                    },
                    task,
                )?,
                CapabilityKind::Completion => validate_completion(
                    channel,
                    Completion {
                        channel: item.channel,
                        owner: task,
                        token: item.token,
                    },
                    task,
                )?,
            }
            if !distinct
                .iter()
                .any(|seen| seen.channel == item.channel && seen.kind == item.kind)
            {
                distinct.push(*item);
            }
        }
        Ok(distinct)
    }

    /// Puts each carried capability in flight in `value`'s parcel, with its
    /// next token.
    fn dispatch_parcel(&mut self, task: TaskId, value: usize, carried: Vec<Carried>) {
        for item in &carried {
            let channel = self.channels.get_mut(&item.channel).expect("validated");
            let (owner, token, parcel) = channel.holder(item.kind);
            *owner = None;
            *token = token.checked_add(1).expect("capability token exhausted");
            *parcel = Some(value);
            self.refresh_owner_index(item.channel, task);
        }
        let previous = self.parcels.insert(
            value,
            Parcel {
                sender: task,
                carried,
            },
        );
        debug_assert!(previous.is_none(), "a held value is sent once");
    }

    /// Moves the capabilities of every value the transition delivered,
    /// handed back, or discarded: to its receiver, back to its sender with
    /// the sender's tokens, or out of the program by aborting their
    /// channels. An abort can discard or hand back further values, so this
    /// runs until the transition holds no unsettled parcel.
    fn settle_parcels(&mut self, transition: &mut Transition) {
        let mut discarded = 0;
        loop {
            if self.parcels.is_empty() {
                transition.delivered.clear();
                transition.returned.clear();
                return;
            }
            for (value, receiver) in std::mem::take(&mut transition.delivered) {
                if let Some(parcel) = self.parcels.remove(&value) {
                    for item in parcel.carried {
                        self.land(item, receiver, None);
                    }
                }
            }
            for value in std::mem::take(&mut transition.returned) {
                if let Some(parcel) = self.parcels.remove(&value) {
                    for item in parcel.carried {
                        self.land(item, parcel.sender, Some(item.token));
                    }
                }
            }
            if discarded == transition.discarded.len() {
                return;
            }
            let value = transition.discarded[discarded];
            discarded += 1;
            if let Some(parcel) = self.parcels.remove(&value) {
                for item in parcel.carried {
                    let released = self.release_lost(item);
                    transition.merge(released);
                }
            }
        }
    }

    /// Gives an in-flight capability to `owner`, restoring `token` when it
    /// goes back to its sender.
    fn land(&mut self, item: Carried, owner: TaskId, token: Option<u64>) {
        let Some(channel) = self.channels.get_mut(&item.channel) else {
            return;
        };
        let (holder, current, parcel) = channel.holder(item.kind);
        *holder = Some(owner);
        *parcel = None;
        if let Some(token) = token {
            *current = token;
        }
        self.refresh_owner_index(item.channel, owner);
    }

    /// Ends an in-flight capability whose value was discarded, as its
    /// owner's exit would (`cleanup_task`): an open receiver or completion,
    /// or a closed receiver with values still queued, aborts its channel.
    fn release_lost(&mut self, item: Carried) -> Transition {
        let Some(channel) = self.channels.get_mut(&item.channel) else {
            return Transition::default();
        };
        let (_, _, parcel) = channel.holder(item.kind);
        *parcel = None;
        let should_abort = match item.kind {
            CapabilityKind::Receiver => {
                channel.lifecycle == Lifecycle::Open
                    || (channel.lifecycle == Lifecycle::Closed && !channel.queue.is_empty())
            }
            CapabilityKind::Completion => channel.lifecycle == Lifecycle::Open,
        };
        if should_abort {
            self.abort_channel(item.channel, None, ChannelEventKind::Aborted)
        } else {
            Transition::default()
        }
    }

    fn refresh_owner_index(&mut self, id: ChannelId, owner: TaskId) {
        let owns_channel = self.channels.get(&id).is_some_and(|channel| {
            channel.receiver_owner == Some(owner) || channel.completion_owner == Some(owner)
        });
        if owns_channel {
            self.owned_channels.entry(owner).or_default().insert(id);
            return;
        }
        let remove_owner = self.owned_channels.get_mut(&owner).is_some_and(|channels| {
            channels.remove(&id);
            channels.is_empty()
        });
        if remove_owner {
            self.owned_channels.remove(&owner);
        }
    }

    fn begin_operation(&self, task: TaskId) -> Result<(), ChannelError> {
        if self.operations.contains_key(&task) {
            return Err(ChannelError::OverlappingOperation(task));
        }
        Ok(())
    }

    fn cancel_operation(&mut self, task: TaskId) -> Transition {
        let mut transition = Transition::default();
        let Some(operation) = self.operations.remove(&task) else {
            return transition;
        };
        match operation {
            Operation::Sending { channel } => {
                let channel_state = self.channels.get_mut(&channel).expect("pending channel");
                if let Some(index) = channel_state
                    .waiting_senders
                    .iter()
                    .position(|waiter| waiter.task == task)
                    && let Some(waiter) = channel_state.waiting_senders.remove(index)
                {
                    transition.discarded.push(waiter.value);
                }
                observe_channel(
                    &mut transition,
                    self.observe_full,
                    channel_state,
                    ChannelObservation {
                        channel,
                        task: Some(task),
                        kind: ChannelEventKind::SendCancelled,
                        cause: Some(task),
                        discard_count: 0,
                    },
                );
                channel_state.assert_invariants();
            }
            Operation::Receiving { channel } => {
                let channel_state = self.channels.get_mut(&channel).expect("pending channel");
                if channel_state.waiting_receiver == Some(task) {
                    channel_state.waiting_receiver = None;
                }
                observe_channel(
                    &mut transition,
                    self.observe_full,
                    channel_state,
                    ChannelObservation {
                        channel,
                        task: Some(task),
                        kind: ChannelEventKind::ReceiveCancelled,
                        cause: Some(task),
                        discard_count: 0,
                    },
                );
                channel_state.assert_invariants();
            }
            ready @ (Operation::SendReady(_) | Operation::ReceiveReady(_)) => {
                transition.discarded.extend(ready.held_value());
            }
        }
        transition
    }

    /// Aborts the channel and records `summary` (Aborted or Released) for
    /// it. Aborting an aborted channel again records nothing.
    fn abort_channel(
        &mut self,
        id: ChannelId,
        cause: Option<TaskId>,
        summary: ChannelEventKind,
    ) -> Transition {
        let observe_boundaries = self.observe_boundaries;
        let channel = self.channels.get_mut(&id).expect("known channel");
        let mut transition = Transition::default();
        if channel.lifecycle == Lifecycle::Aborted && summary == ChannelEventKind::Aborted {
            return transition;
        }
        channel.lifecycle = Lifecycle::Aborted;
        let discard_count = channel.queue.len();
        transition.discarded.extend(channel.queue.drain(..));
        while let Some(waiter) = channel.waiting_senders.pop_front() {
            self.operations.insert(
                waiter.task,
                Operation::SendReady(SendResult::Aborted(waiter.value)),
            );
            transition.returned.push(waiter.value);
            transition.wakes.push(waiter.task);
            observe_channel(
                &mut transition,
                observe_boundaries,
                channel,
                ChannelObservation {
                    channel: id,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendRejectedAborted,
                    cause,
                    discard_count: 0,
                },
            );
        }
        if let Some(receiver) = channel.waiting_receiver.take() {
            self.operations
                .insert(receiver, Operation::ReceiveReady(ReceiveResult::Aborted));
            transition.wakes.push(receiver);
            observe_channel(
                &mut transition,
                observe_boundaries,
                channel,
                ChannelObservation {
                    channel: id,
                    task: Some(receiver),
                    kind: ChannelEventKind::Aborted,
                    cause,
                    discard_count: 0,
                },
            );
        }
        observe_channel(
            &mut transition,
            observe_boundaries,
            channel,
            ChannelObservation {
                channel: id,
                task: cause,
                kind: summary,
                cause,
                discard_count,
            },
        );
        channel.assert_invariants();
        transition
    }
}

impl Transition {
    fn merge(&mut self, mut other: Self) {
        self.wakes.append(&mut other.wakes);
        self.observations.append(&mut other.observations);
        self.discarded.append(&mut other.discarded);
        self.delivered.append(&mut other.delivered);
        self.returned.append(&mut other.returned);
    }
}

impl Operation {
    /// The value a ready result carries back to its task: a received
    /// message, or a refused send's own value.
    fn held_value(self) -> Option<usize> {
        match self {
            Self::SendReady(SendResult::Closed(value) | SendResult::Aborted(value))
            | Self::ReceiveReady(ReceiveResult::Received(value)) => Some(value),
            Self::Sending { .. }
            | Self::SendReady(SendResult::Pending | SendResult::Accepted)
            | Self::Receiving { .. }
            | Self::ReceiveReady(
                ReceiveResult::Pending | ReceiveResult::Drained | ReceiveResult::Aborted,
            ) => None,
        }
    }
}

impl Channel {
    /// The owner, token, and parcel of one of the channel's capabilities.
    fn holder(&mut self, kind: CapabilityKind) -> (&mut Option<TaskId>, &mut u64, &mut Option<usize>) {
        match kind {
            CapabilityKind::Receiver => (
                &mut self.receiver_owner,
                &mut self.receiver_token,
                &mut self.receiver_parcel,
            ),
            CapabilityKind::Completion => (
                &mut self.completion_owner,
                &mut self.completion_token,
                &mut self.completion_parcel,
            ),
        }
    }

    fn assert_invariants(&self) {
        debug_assert!(self.receiver_parcel.is_none() || self.receiver_owner.is_none());
        debug_assert!(self.completion_parcel.is_none() || self.completion_owner.is_none());
        debug_assert!(self.queue.len() <= self.capacity);
        if self.waiting_receiver.is_some() {
            debug_assert!(self.queue.is_empty());
            debug_assert!(self.waiting_senders.is_empty());
        }
        if !self.waiting_senders.is_empty() {
            debug_assert_eq!(self.queue.len(), self.capacity);
            debug_assert_eq!(self.lifecycle, Lifecycle::Open);
        }
        if self.lifecycle == Lifecycle::Aborted {
            debug_assert!(self.queue.is_empty());
            debug_assert!(self.waiting_senders.is_empty());
            debug_assert!(self.waiting_receiver.is_none());
        }
    }
}

// The token alone names an ownership epoch: every transfer, send, and delivery
// that changes the owner moves it on (a hand-back restores the sender's own), so a
// copy's `owner` field is not compared. A received copy still carries its
// sender's id there.
fn validate_receiver(
    channel: &Channel,
    receiver: Receiver,
    task: TaskId,
) -> Result<(), ChannelError> {
    if channel.receiver_parcel.is_some() {
        return Err(ChannelError::CapabilityInFlight {
            channel: receiver.channel,
            kind: CapabilityKind::Receiver,
        });
    }
    if channel.receiver_owner != Some(task) || receiver.token != channel.receiver_token {
        return Err(ChannelError::WrongReceiverOwner {
            channel: receiver.channel,
            expected: channel.receiver_owner,
            actual: task,
        });
    }
    Ok(())
}

fn validate_completion(
    channel: &Channel,
    completion: Completion,
    task: TaskId,
) -> Result<(), ChannelError> {
    if channel.completion_parcel.is_some() {
        return Err(ChannelError::CapabilityInFlight {
            channel: completion.channel,
            kind: CapabilityKind::Completion,
        });
    }
    if channel.completion_owner != Some(task) || completion.token != channel.completion_token {
        return Err(ChannelError::WrongCompletionOwner {
            channel: completion.channel,
            expected: channel.completion_owner,
            actual: task,
        });
    }
    Ok(())
}

fn observe_channel(
    transition: &mut Transition,
    enabled: bool,
    state: &Channel,
    event: ChannelObservation,
) {
    record(
        transition,
        enabled,
        Observation {
            channel: event.channel,
            task: event.task,
            kind: event.kind,
            cause: event.cause,
            capacity: state.capacity,
            queue_depth: state.queue.len(),
            waiter_depth: state.waiting_senders.len()
                + usize::from(state.waiting_receiver.is_some()),
            discard_count: event.discard_count,
        },
    );
}

fn record(transition: &mut Transition, enabled: bool, observation: Observation) {
    if enabled {
        transition.observations.push(observation);
    }
}

#[cfg(test)]
mod parcel_tests {
    use super::*;

    const PARENT: TaskId = TaskId::from_raw(1);
    const CHILD: TaskId = TaskId::from_raw(2);
    const MESSAGE: usize = 0xbeef;

    fn registry() -> ChannelRegistry {
        ChannelRegistry::new(true, true)
    }

    fn carried_receiver(capabilities: ChannelCapabilities) -> Carried {
        Carried {
            channel: capabilities.receiver.channel(),
            kind: CapabilityKind::Receiver,
            token: capabilities.receiver.token(),
        }
    }

    fn receiver_at(capabilities: ChannelCapabilities, owner: TaskId, token: u64) -> Receiver {
        Receiver::from_raw(capabilities.receiver.channel().get(), owner.get(), token)
    }

    fn aborted(transition: &Transition, channel: ChannelId) -> bool {
        transition
            .observations
            .iter()
            .any(|seen| seen.channel == channel && seen.kind == ChannelEventKind::Aborted)
    }

    /// CHILD sends PARENT a message on `route` (owned by PARENT) carrying the
    /// Receiver of `carried`, which CHILD owns.
    fn child_sends_receiver(
        channels: &mut ChannelRegistry,
        route_capacity: usize,
    ) -> (ChannelCapabilities, ChannelCapabilities, SendResult) {
        let (route, _) = channels.create(PARENT, route_capacity);
        let (carried, _) = channels.create(CHILD, 1);
        let (result, _) = channels
            .send_carrying(CHILD, route.sender, MESSAGE, &[carried_receiver(carried)])
            .expect("CHILD owns the carried Receiver");
        (route, carried, result)
    }

    #[test]
    fn an_unreceived_capability_is_in_flight_and_the_receiver_gets_the_next_token() {
        let mut channels = registry();
        let (route, carried, result) = child_sends_receiver(&mut channels, 1);
        assert_eq!(result, SendResult::Accepted);
        channels.take_send_result(CHILD);
        assert_eq!(
            channels
                .receive(CHILD, carried.receiver)
                .map(|(result, _)| result),
            Err(ChannelError::CapabilityInFlight {
                channel: carried.receiver.channel(),
                kind: CapabilityKind::Receiver,
            })
        );

        let (received, _) = channels.receive(PARENT, route.receiver).unwrap();
        assert_eq!(received, ReceiveResult::Received(MESSAGE));
        channels.take_receive_result(PARENT);
        assert!(matches!(
            channels.receive(PARENT, receiver_at(carried, PARENT, 1)),
            Err(ChannelError::WrongReceiverOwner { .. })
        ));
        let (waiting, _) = channels
            .receive(PARENT, receiver_at(carried, PARENT, 2))
            .unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    #[test]
    fn a_waiting_receiver_owns_the_capability_on_delivery() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 0);
        let (carried, _) = channels.create(CHILD, 1);
        let (parked, _) = channels.receive(PARENT, route.receiver).unwrap();
        assert_eq!(parked, ReceiveResult::Pending);
        channels
            .send_carrying(CHILD, route.sender, MESSAGE, &[carried_receiver(carried)])
            .unwrap();
        assert_eq!(
            channels.take_receive_result(PARENT),
            Some(ReceiveResult::Received(MESSAGE))
        );
        let (waiting, _) = channels
            .receive(PARENT, receiver_at(carried, PARENT, 2))
            .unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    #[test]
    fn a_parked_send_refused_by_close_hands_the_capability_back() {
        let mut channels = registry();
        let (route, carried, result) = child_sends_receiver(&mut channels, 0);
        assert_eq!(result, SendResult::Pending);
        channels.close(PARENT, route.completion).unwrap();
        assert_eq!(
            channels.take_send_result(CHILD),
            Some(SendResult::Closed(MESSAGE))
        );
        let (waiting, _) = channels.receive(CHILD, carried.receiver).unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    #[test]
    fn a_handed_back_capability_can_be_sent_again() {
        let mut channels = registry();
        let (route, carried, _) = child_sends_receiver(&mut channels, 0);
        channels.close(PARENT, route.completion).unwrap();
        channels.take_send_result(CHILD);
        let (again, _) = channels.create(PARENT, 1);
        let (result, _) = channels
            .send_carrying(CHILD, again.sender, MESSAGE, &[carried_receiver(carried)])
            .unwrap();
        assert_eq!(result, SendResult::Accepted);
        channels.receive(PARENT, again.receiver).unwrap();
        channels.take_receive_result(PARENT);
        let (waiting, _) = channels
            .receive(PARENT, receiver_at(carried, PARENT, 2))
            .unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    #[test]
    fn a_cancelled_parked_send_aborts_the_carried_channel() {
        let mut channels = registry();
        let (_, carried, result) = child_sends_receiver(&mut channels, 0);
        assert_eq!(result, SendResult::Pending);
        let transition = channels.cancel_pending(CHILD);
        assert!(aborted(&transition, carried.receiver.channel()));
        assert_eq!(transition.discarded, vec![MESSAGE]);
    }

    #[test]
    fn a_discarded_completion_aborts_its_channel_and_its_receiver_sees_it() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (carried, _) = channels.create(CHILD, 1);
        let completion = Carried {
            channel: carried.completion.channel(),
            kind: CapabilityKind::Completion,
            token: carried.completion.token(),
        };
        channels
            .send_carrying(CHILD, route.sender, MESSAGE, &[completion])
            .unwrap();
        channels.take_send_result(CHILD);
        let transition = channels.abort(PARENT, route.receiver).unwrap();
        assert!(aborted(&transition, carried.completion.channel()));
        let (seen, _) = channels.receive(CHILD, carried.receiver).unwrap();
        assert_eq!(seen, ReceiveResult::Aborted);
    }

    #[test]
    fn a_capability_named_twice_in_one_message_moves_once() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (carried, _) = channels.create(CHILD, 1);
        let item = carried_receiver(carried);
        channels
            .send_carrying(CHILD, route.sender, MESSAGE, &[item, item])
            .unwrap();
        channels.take_send_result(CHILD);
        channels.receive(PARENT, route.receiver).unwrap();
        channels.take_receive_result(PARENT);
        let (waiting, _) = channels
            .receive(PARENT, receiver_at(carried, PARENT, 2))
            .unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    #[test]
    fn a_send_carrying_someone_elses_capability_changes_nothing() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (carried, _) = channels.create(CHILD, 1);
        assert!(matches!(
            channels.send_carrying(PARENT, route.sender, MESSAGE, &[carried_receiver(carried)]),
            Err(ChannelError::WrongReceiverOwner { .. })
        ));
        assert_eq!(channels.metrics(route.sender.channel()).unwrap().queue_depth, 0);
        let (waiting, _) = channels.receive(CHILD, carried.receiver).unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }
}
