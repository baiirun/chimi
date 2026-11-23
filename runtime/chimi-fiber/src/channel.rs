use crate::scheduler::TaskId;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
    /// A broadcast publish was accepted and reached this many subscriptions.
    Published(usize),
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
    /// A broadcast or watch value holds an exclusive capability, which every
    /// subscriber's copy would duplicate.
    PublishedCapability(ChannelId),
    /// The request named a channel of another kind: a subscribe to a plain
    /// channel, or a send to a receiver's entry.
    WrongKind(ChannelId),
    /// A send carried the Receiver of its own channel, so the message could
    /// never be received.
    OwnReceiver(ChannelId),
}

/// What a broadcast or watch entry (a hub) gives its subscriptions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HubKind {
    /// Every publish, to every subscription, once all have room.
    Broadcast,
    /// The latest value; a publish replaces an unread one.
    Watch,
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

/// A broadcast or a watch. Its entry holds no values of its own except a
/// watch's current one; its `waiting_senders` are parked publishes.
struct Hub {
    kind: HubKind,
    /// The subscription queues it feeds, in subscription order. A
    /// subscription leaves when its last receiver ends, and all leave when
    /// the hub closes or aborts.
    subscriptions: Vec<ChannelId>,
    /// A watch's current value, which a new subscription receives a copy of.
    current: Option<usize>,
}

/// One registry entry. Every entry is one receiver (unless it is a hub), so
/// owner tokens, transfers and parcels work per receiver. A queue entry also
/// holds the values; a tap (`Channel.add_receiver`) receives from the queue
/// named by `tap_of`; a hub feeds subscription queues.
struct Channel {
    capacity: usize,
    queue: VecDeque<usize>,
    lifecycle: Lifecycle,
    waiting_senders: VecDeque<WaitingSend>,
    /// Tasks parked in a receive on this queue, served in order.
    waiting_receivers: VecDeque<TaskId>,
    receiver_owner: Option<TaskId>,
    receiver_token: u64,
    /// This entry's receiver has ended (aborted, or abandoned by its owner or
    /// its message): it answers `Aborted`, and no longer keeps its queue open.
    receiver_ended: bool,
    completion_owner: Option<TaskId>,
    completion_token: u64,
    /// When the current owner of each capability gained it, on the
    /// registry's arrival clock: a cancelled timeout scope aborts what its
    /// task gained after the scope began (`abort_arrivals`).
    receiver_since: u64,
    completion_since: u64,
    /// The sent value whose parcel holds the receiver or the completion. A
    /// capability in flight has no owner.
    receiver_parcel: Option<usize>,
    completion_parcel: Option<usize>,
    /// Receivers' owners waiting in `Select.first` with an event on this
    /// queue. They are woken, not answered, when the queue turns ready to
    /// receive, and each receives only if it still is when it polls again.
    select_watchers: Vec<TaskId>,
    /// For a tap, the queue it receives from.
    tap_of: Option<ChannelId>,
    /// For a queue, the taps added to it.
    taps: Vec<ChannelId>,
    /// For a subscription queue, the hub that feeds it while it is open.
    feed: Option<ChannelId>,
    /// A watch subscription: a value placed into a full queue replaces the
    /// unread one.
    conflate: bool,
    /// The host released this queue while taps still receive from it.
    released: bool,
    hub: Option<Hub>,
}

impl Channel {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
            lifecycle: Lifecycle::Open,
            waiting_senders: VecDeque::new(),
            waiting_receivers: VecDeque::new(),
            receiver_owner: None,
            receiver_token: 1,
            receiver_ended: false,
            completion_owner: None,
            completion_token: 1,
            receiver_since: 0,
            completion_since: 0,
            receiver_parcel: None,
            completion_parcel: None,
            select_watchers: Vec::new(),
            tap_of: None,
            taps: Vec::new(),
            feed: None,
            conflate: false,
            released: false,
            hub: None,
        }
    }

    /// An open queue that is not a hub, a tap, a subscription or
    /// conflating, with no select watching it: the channels the local path
    /// serves.
    fn is_plain_open_queue(&self) -> bool {
        self.lifecycle == Lifecycle::Open
            && self.hub.is_none()
            && self.tap_of.is_none()
            && self.feed.is_none()
            && !self.conflate
            && self.select_watchers.is_empty()
    }

    fn has_room(&self) -> bool {
        !self.waiting_receivers.is_empty() || self.queue.len() < self.capacity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Sending { channel: ChannelId },
    SendReady(SendResult),
    /// `channel` is the queue the task waits on (a tap's queue, not the tap).
    Receiving { channel: ChannelId },
    ReceiveReady(ReceiveResult),
}

/// Duplicates a queued value for another subscription (the host's
/// `copy_channel_value`).
pub(crate) type ValueCopier = Box<dyn FnMut(usize) -> usize>;

pub(crate) struct ChannelRegistry {
    /// Each homed on the carrier that made it, for good.
    channels: crate::scheduler::Homed<ChannelId, Channel>,
    /// The carrier holding the kernel, which new channels are homed on
    /// (`Scheduler::enter_carrier` sets it).
    carrier: crate::scheduler::Carrier,
    owned_channels: BTreeMap<TaskId, BTreeSet<ChannelId>>,
    operations: Operations,
    /// Keyed by the sent value, which is unique while the registry holds it.
    parcels: BTreeMap<usize, Parcel>,
    next_channel: u64,
    /// Counts ownership changes; see `Channel::receiver_since`.
    arrivals: u64,
    observe_boundaries: bool,
    observe_full: bool,
    /// Without a copier, every subscription gets the same value (tests whose
    /// values are plain numbers).
    copier: Option<ValueCopier>,
}

impl ChannelRegistry {
    pub fn new(observe_boundaries: bool, observe_full: bool) -> Self {
        Self {
            channels: crate::scheduler::Homed::default(),
            carrier: crate::scheduler::Carrier::default(),
            owned_channels: BTreeMap::new(),
            operations: Operations::default(),
            parcels: BTreeMap::new(),
            next_channel: 1,
            arrivals: 0,
            observe_boundaries,
            observe_full,
            copier: None,
        }
    }

    /// Homes the channels made from now on on `carrier`.
    pub(crate) fn enter_carrier(&mut self, carrier: crate::scheduler::Carrier) {
        self.carrier = carrier;
    }

    /// True when no capability is in flight in any value.
    pub(crate) fn parcels_empty(&self) -> bool {
        self.parcels.is_empty()
    }

    pub fn set_copier(&mut self, copier: ValueCopier) {
        self.copier = Some(copier);
    }

    fn copy_value(&mut self, value: usize) -> usize {
        match &mut self.copier {
            Some(copy) => copy(value),
            None => value,
        }
    }

    fn next_id(&mut self) -> ChannelId {
        let id = ChannelId(self.next_channel);
        self.next_channel = self
            .next_channel
            .checked_add(1)
            .expect("channel identity space exhausted");
        id
    }

    fn created(&self, id: ChannelId, owner: TaskId, capacity: usize) -> Transition {
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
        transition
    }

    pub fn create(&mut self, owner: TaskId, capacity: usize) -> (ChannelCapabilities, Transition) {
        let id = self.next_id();
        let mut channel = Channel::new(capacity);
        channel.receiver_owner = Some(owner);
        channel.completion_owner = Some(owner);
        let since = self.tick();
        channel.receiver_since = since;
        channel.completion_since = since;
        self.channels.insert(id, self.carrier, channel);
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
        (capabilities, self.created(id, owner, capacity))
    }

    /// Creates a broadcast (subscriptions of `capacity` values each) or a
    /// watch whose current value is `initial`. Returns its publisher (a
    /// Sender of the hub) and its completion, owned by `owner`.
    pub fn create_hub(
        &mut self,
        owner: TaskId,
        kind: HubKind,
        capacity: usize,
        initial: Option<usize>,
    ) -> (Sender, Completion, Transition) {
        let id = self.next_id();
        let mut channel = Channel::new(capacity);
        channel.receiver_ended = true;
        channel.completion_owner = Some(owner);
        channel.completion_since = self.tick();
        channel.hub = Some(Hub {
            kind,
            subscriptions: Vec::new(),
            current: initial,
        });
        self.channels.insert(id, self.carrier, channel);
        self.owned_channels.entry(owner).or_default().insert(id);
        let completion = Completion {
            channel: id,
            owner,
            token: 1,
        };
        (Sender { channel: id }, completion, self.created(id, owner, capacity))
    }

    /// A new subscription of the hub `publisher` names, owned by `task`: a
    /// queue that receives what the hub publishes from now on (a watch's
    /// starts with a copy of its current value). It starts closed or aborted
    /// when the hub is.
    pub fn subscribe(
        &mut self,
        task: TaskId,
        publisher: Sender,
    ) -> Result<(Receiver, Transition), ChannelError> {
        let hub_id = publisher.channel;
        let hub = self
            .channels
            .get(&hub_id)
            .ok_or(ChannelError::UnknownChannel(hub_id))?;
        let (kind, current) = match &hub.hub {
            Some(state) => (state.kind, state.current),
            None => return Err(ChannelError::WrongKind(hub_id)),
        };
        let lifecycle = hub.lifecycle;
        let capacity = match kind {
            HubKind::Broadcast => hub.capacity,
            HubKind::Watch => 1,
        };
        let id = self.next_id();
        let mut channel = Channel::new(capacity);
        channel.receiver_owner = Some(task);
        channel.receiver_since = self.tick();
        channel.lifecycle = lifecycle;
        channel.conflate = kind == HubKind::Watch;
        if lifecycle != Lifecycle::Aborted
            && kind == HubKind::Watch
            && let Some(current) = current
        {
            let copy = self.copy_value(current);
            channel.queue.push_back(copy);
        }
        if lifecycle == Lifecycle::Open {
            channel.feed = Some(hub_id);
            self.channels
                .get_mut(&hub_id)
                .and_then(|hub| hub.hub.as_mut())
                .expect("hub")
                .subscriptions
                .push(id);
        }
        self.channels.insert(id, self.carrier, channel);
        self.owned_channels.entry(task).or_default().insert(id);
        let receiver = Receiver {
            channel: id,
            owner: task,
            token: 1,
        };
        Ok((receiver, self.created(id, task, capacity)))
    }

    /// Another receiver of the queue `receiver` receives from, owned by
    /// `task`. It is ended at once when `receiver` has ended or the queue is
    /// aborted. `receiver` must be `task`'s current copy.
    pub fn add_receiver(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<(Receiver, Transition), ChannelError> {
        let entry = self
            .channels
            .get(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(entry, receiver, task)?;
        let ended = entry.receiver_ended;
        let queue_id = self.queue_id(receiver.channel);
        let queue = self.channels.get(&queue_id).expect("tap's queue");
        let ended = ended || queue.lifecycle == Lifecycle::Aborted;
        let capacity = queue.capacity;
        let id = self.next_id();
        let mut tap = Channel::new(0);
        tap.receiver_owner = Some(task);
        tap.receiver_since = self.tick();
        tap.receiver_ended = ended;
        tap.tap_of = Some(queue_id);
        self.channels.insert(id, self.carrier, tap);
        self.channels
            .get_mut(&queue_id)
            .expect("tap's queue")
            .taps
            .push(id);
        self.owned_channels.entry(task).or_default().insert(id);
        let added = Receiver {
            channel: id,
            owner: task,
            token: 1,
        };
        Ok((added, self.created(id, task, capacity)))
    }

    /// The queue a receiver entry receives from: a tap's queue, or itself.
    fn queue_id(&self, id: ChannelId) -> ChannelId {
        self.channels
            .get(&id)
            .and_then(|channel| channel.tap_of)
            .unwrap_or(id)
    }

    /// Receivers of `queue` that have not ended: its own and its taps'. A
    /// receiver in flight in a message counts.
    fn live_receivers(&self, queue: ChannelId) -> usize {
        let Some(channel) = self.channels.get(&queue) else {
            return 0;
        };
        usize::from(!channel.receiver_ended)
            + channel
                .taps
                .iter()
                .filter(|tap| self.channels.get(tap).is_some_and(|tap| !tap.receiver_ended))
                .count()
    }

    /// Whether `carried` holds every live receiver of the open `queue`: its
    /// own and its taps'. A message on `queue` that carries them could never
    /// be received, since only those receivers read it.
    fn carries_every_receiver(&self, queue: ChannelId, carried: &[Carried]) -> bool {
        let carries = |id: ChannelId| {
            carried
                .iter()
                .any(|item| item.kind == CapabilityKind::Receiver && item.channel == id)
        };
        let Some(channel) = self.channels.get(&queue) else {
            return false;
        };
        // A closed or aborted channel refuses the send and hands the
        // capabilities back, so nothing is stranded.
        if channel.lifecycle != Lifecycle::Open {
            return false;
        }
        let mut live = channel
            .taps
            .iter()
            .copied()
            .filter(|tap| self.channels.get(tap).is_some_and(|tap| !tap.receiver_ended))
            .chain((!channel.receiver_ended).then_some(queue))
            .peekable();
        live.peek().is_some() && live.all(carries)
    }

    /// Sends `value`, which holds the `carried` capabilities of `task`. Once
    /// accepted or parked, each is in flight with its next token until the
    /// value reaches a receiver, who then owns it; a refused value hands
    /// them back with their old tokens, and a discarded one ends them, as
    /// its owner's exit would have. Nothing changes when any carried
    /// capability is not `task`'s. A send to a hub publishes; its value must
    /// carry nothing.
    pub fn send_carrying(
        &mut self,
        task: TaskId,
        sender: Sender,
        value: usize,
        carried: &[Carried],
    ) -> Result<(SendResult, Transition), ChannelError> {
        self.begin_operation(task)?;
        let target = self
            .channels
            .get(&sender.channel)
            .ok_or(ChannelError::UnknownChannel(sender.channel))?;
        if target.tap_of.is_some() || target.feed.is_some() {
            return Err(ChannelError::WrongKind(sender.channel));
        }
        let is_hub = target.hub.is_some();
        let carried = self.validate_carried(task, carried)?;
        if is_hub {
            if !carried.is_empty() {
                return Err(ChannelError::PublishedCapability(sender.channel));
            }
            return Ok(self.publish(task, sender.channel, value));
        }
        if self.carries_every_receiver(sender.channel, &carried) {
            return Err(ChannelError::OwnReceiver(sender.channel));
        }
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
        let observe_full = self.observe_full;
        let channel = self
            .channels
            .get_mut(&sender.channel)
            .ok_or(ChannelError::UnknownChannel(sender.channel))?;
        let mut transition = Transition::default();

        let result = match channel.lifecycle {
            Lifecycle::Closed => SendResult::Closed(value),
            Lifecycle::Aborted => SendResult::Aborted(value),
            Lifecycle::Open if channel.has_room() => {
                debug_assert!(channel.waiting_senders.is_empty());
                place(
                    channel,
                    sender.channel,
                    &mut self.operations,
                    value,
                    Some(task),
                    observe_full,
                    &mut transition,
                );
                SendResult::Accepted
            }
            Lifecycle::Open => {
                channel
                    .waiting_senders
                    .push_back(WaitingSend { task, value });
                transition
                    .wakes
                    .extend(std::mem::take(&mut channel.select_watchers));
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
        };
        self.finish_send(task, sender.channel, result, &mut transition);
        Ok((result, transition))
    }

    /// Records a send's (or publish's) immediate result for `task`.
    fn finish_send(
        &mut self,
        task: TaskId,
        id: ChannelId,
        result: SendResult,
        transition: &mut Transition,
    ) {
        self.operations.insert(task, Operation::SendReady(result));
        transition.wakes.push(task);
        if let SendResult::Closed(value) | SendResult::Aborted(value) = result {
            transition.returned.push(value);
        }
        let (kind, enabled) = match result {
            SendResult::Accepted | SendResult::Published(_) => {
                (ChannelEventKind::SendAccepted, self.observe_full)
            }
            SendResult::Closed(_) => (ChannelEventKind::SendRejectedClosed, self.observe_boundaries),
            SendResult::Aborted(_) => {
                (ChannelEventKind::SendRejectedAborted, self.observe_boundaries)
            }
            SendResult::Pending => unreachable!("a pending send has no result yet"),
        };
        let channel = self.channels.get(&id).expect("sent-to channel");
        observe_channel(
            transition,
            enabled,
            channel,
            ChannelObservation {
                channel: id,
                task: Some(task),
                kind,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
    }

    /// A send to the hub `id`. A watch replaces its current value and every
    /// subscription's unread one. A broadcast is accepted when no earlier
    /// publish is parked and every subscription has room, and then reaches
    /// each with its own copy; otherwise it parks.
    fn publish(&mut self, task: TaskId, id: ChannelId, value: usize) -> (SendResult, Transition) {
        let mut transition = Transition::default();
        let hub = self.channels.get(&id).expect("hub");
        let kind = hub.hub.as_ref().expect("hub").kind;
        let result = match hub.lifecycle {
            Lifecycle::Closed => SendResult::Closed(value),
            Lifecycle::Aborted => SendResult::Aborted(value),
            Lifecycle::Open => match kind {
                HubKind::Watch => {
                    let subscriptions = hub.hub.as_ref().expect("hub").subscriptions.clone();
                    for subscription in subscriptions {
                        let copy = self.copy_value(value);
                        self.place_into(subscription, copy, Some(task), &mut transition);
                    }
                    let state = self
                        .channels
                        .get_mut(&id)
                        .and_then(|hub| hub.hub.as_mut())
                        .expect("hub");
                    transition.discarded.extend(state.current.replace(value));
                    SendResult::Accepted
                }
                HubKind::Broadcast => {
                    if hub.waiting_senders.is_empty() && self.subscriptions_have_room(id) {
                        SendResult::Published(self.broadcast(id, value, task, &mut transition))
                    } else {
                        let observe_full = self.observe_full;
                        let hub = self.channels.get_mut(&id).expect("hub");
                        hub.waiting_senders.push_back(WaitingSend { task, value });
                        self.operations
                            .insert(task, Operation::Sending { channel: id });
                        observe_channel(
                            &mut transition,
                            observe_full,
                            hub,
                            ChannelObservation {
                                channel: id,
                                task: Some(task),
                                kind: ChannelEventKind::SendParked,
                                cause: None,
                                discard_count: 0,
                            },
                        );
                        return (SendResult::Pending, transition);
                    }
                }
            },
        };
        self.finish_send(task, id, result, &mut transition);
        (result, transition)
    }

    fn subscriptions_have_room(&self, hub: ChannelId) -> bool {
        self.channels[&hub]
            .hub
            .as_ref()
            .expect("hub")
            .subscriptions
            .iter()
            .all(|subscription| self.channels[subscription].has_room())
    }

    /// Places `value` into every subscription of the broadcast `hub`, which
    /// all have room, and returns how many there were. The last one gets
    /// `value` itself; with none, `value` is released.
    fn broadcast(
        &mut self,
        hub: ChannelId,
        value: usize,
        cause: TaskId,
        transition: &mut Transition,
    ) -> usize {
        let subscriptions = self.channels[&hub]
            .hub
            .as_ref()
            .expect("hub")
            .subscriptions
            .clone();
        let Some((last, rest)) = subscriptions.split_last() else {
            transition.discarded.push(value);
            return 0;
        };
        for subscription in rest {
            let copy = self.copy_value(value);
            self.place_into(*subscription, copy, Some(cause), transition);
        }
        self.place_into(*last, value, Some(cause), transition);
        subscriptions.len()
    }

    fn place_into(
        &mut self,
        id: ChannelId,
        value: usize,
        cause: Option<TaskId>,
        transition: &mut Transition,
    ) {
        let observe_full = self.observe_full;
        let channel = self.channels.get_mut(&id).expect("subscription");
        place(
            channel,
            id,
            &mut self.operations,
            value,
            cause,
            observe_full,
            transition,
        );
    }

    /// Accepts parked publishes of the broadcast `hub`, oldest first, while
    /// every subscription has room.
    fn admit_publishers(&mut self, hub: ChannelId, transition: &mut Transition) {
        loop {
            let Some(state) = self.channels.get(&hub) else {
                return;
            };
            if state.lifecycle != Lifecycle::Open || state.waiting_senders.is_empty() {
                return;
            }
            if !self.subscriptions_have_room(hub) {
                return;
            }
            let waiter = self
                .channels
                .get_mut(&hub)
                .expect("hub")
                .waiting_senders
                .pop_front()
                .expect("a parked publish");
            let reached = self.broadcast(hub, waiter.value, waiter.task, transition);
            self.operations.insert(
                waiter.task,
                Operation::SendReady(SendResult::Published(reached)),
            );
            transition.wakes.push(waiter.task);
            let state = &self.channels[&hub];
            observe_channel(
                transition,
                self.observe_full,
                state,
                ChannelObservation {
                    channel: hub,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendAccepted,
                    cause: None,
                    discard_count: 0,
                },
            );
        }
    }

    /// `take_send_result`, or else `send_carrying` of a value that carries
    /// nothing, on the local path of step 4e, or None, having changed nothing, when it is not local:
    /// tracing is on, a parcel is in flight anywhere, `task` has an
    /// operation that is not a ready result, the channel is not a plain open queue homed on `carrier`
    /// or has select watchers, a parked receiver it would hand to is not
    /// local, or it would park without an operation slot to record it in.
    /// Some gives the result and the receiver it handed the value to,
    /// which the caller wakes before `task`, as the transition would.
    ///
    /// # Safety
    ///
    /// As for `Homed::homed_mut`, for `carrier`; `is_local` is true only
    /// for tasks homed on `carrier`.
    pub(crate) unsafe fn send_local(
        &self,
        carrier: crate::scheduler::Carrier,
        task: TaskId,
        sender: Sender,
        value: usize,
        is_local: impl Fn(TaskId) -> bool,
    ) -> Option<(SendResult, Option<TaskId>)> {
        if self.observe_boundaries || self.observe_full || !self.parcels.is_empty() {
            return None;
        }
        // SAFETY (each block below): the caller's contract; `task` and any
        // task `is_local` accepts are homed on `carrier`, and no two of the
        // references made here are to the same record at once.
        match unsafe { self.operations.slot_mut(task) } {
            // A reissued send collects the result it was woken for.
            Some(slot) if matches!(slot, Some(Operation::SendReady(_))) => {
                let Some(Operation::SendReady(result)) = slot.take() else {
                    unreachable!("matched a ready result")
                };
                return Some((result, None));
            }
            Some(Some(_)) => return None,
            _ => {}
        }
        let channel = unsafe { self.channels.homed_mut(&sender.channel, carrier) }?;
        if !channel.is_plain_open_queue() {
            return None;
        }
        let result = if let Some(&receiver) = channel.waiting_receivers.front() {
            if !is_local(receiver) {
                return None;
            }
            let slot = unsafe { self.operations.slot_mut(receiver) }?;
            channel.waiting_receivers.pop_front();
            *slot = Some(Operation::ReceiveReady(ReceiveResult::Received(value)));
            (SendResult::Accepted, Some(receiver))
        } else if channel.queue.len() < channel.capacity {
            channel.queue.push_back(value);
            (SendResult::Accepted, None)
        } else {
            let slot = unsafe { self.operations.slot_mut(task) }?;
            channel.waiting_senders.push_back(WaitingSend { task, value });
            *slot = Some(Operation::Sending {
                channel: sender.channel,
            });
            (SendResult::Pending, None)
        };
        channel.assert_invariants();
        Some(result)
    }

    /// `take_receive_result`, or else `receive`, on the local path, or None, having changed nothing: as for
    /// `send_local`, and also when the receiver is not `task`'s own, is a
    /// tap or a subscription, or the queue is closed and empty. Some gives
    /// the result and the parked sender it admitted, which the caller wakes
    /// before `task`.
    ///
    /// # Safety
    ///
    /// As for `send_local`.
    pub(crate) unsafe fn receive_local(
        &self,
        carrier: crate::scheduler::Carrier,
        task: TaskId,
        receiver: Receiver,
        is_local: impl Fn(TaskId) -> bool,
    ) -> Option<(ReceiveResult, Option<TaskId>)> {
        if self.observe_boundaries || self.observe_full || !self.parcels.is_empty() {
            return None;
        }
        // SAFETY (each block below): as in `send_local`.
        match unsafe { self.operations.slot_mut(task) } {
            // A reissued receive collects the result it was woken for.
            Some(slot) if matches!(slot, Some(Operation::ReceiveReady(_))) => {
                let Some(Operation::ReceiveReady(result)) = slot.take() else {
                    unreachable!("matched a ready result")
                };
                return Some((result, None));
            }
            Some(Some(_)) => return None,
            _ => {}
        }
        let channel = unsafe { self.channels.homed_mut(&receiver.channel, carrier) }?;
        if validate_receiver(channel, receiver, task).is_err()
            || channel.receiver_ended
            || !channel.is_plain_open_queue()
        {
            return None;
        }
        let admitted = match channel.waiting_senders.front() {
            Some(waiter) if !is_local(waiter.task) => return None,
            Some(waiter) => Some((waiter.task, unsafe { self.operations.slot_mut(waiter.task) }?)),
            None => None,
        };
        let result = if let Some(value) = channel.queue.pop_front() {
            let woken = admitted.map(|(task, slot)| {
                let waiter = channel.waiting_senders.pop_front().expect("a parked sender");
                channel.queue.push_back(waiter.value);
                *slot = Some(Operation::SendReady(SendResult::Accepted));
                task
            });
            (ReceiveResult::Received(value), woken)
        } else if let Some((_, slot)) = admitted {
            let waiter = channel.waiting_senders.pop_front().expect("a parked sender");
            *slot = Some(Operation::SendReady(SendResult::Accepted));
            (ReceiveResult::Received(waiter.value), Some(waiter.task))
        } else {
            let slot = unsafe { self.operations.slot_mut(task) }?;
            channel.waiting_receivers.push_back(task);
            *slot = Some(Operation::Receiving {
                channel: receiver.channel,
            });
            (ReceiveResult::Pending, None)
        };
        channel.assert_invariants();
        Some(result)
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
        let entry = self
            .channels
            .get(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(entry, receiver, task)?;
        let ended = entry.receiver_ended;
        let queue_id = self.queue_id(receiver.channel);
        let channel = self.channels.get_mut(&queue_id).expect("receiver's queue");
        let mut transition = Transition::default();

        let result = if ended || channel.lifecycle == Lifecycle::Aborted {
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
                        channel: queue_id,
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
                    channel: queue_id,
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
            channel.waiting_receivers.push_back(task);
            self.operations
                .insert(task, Operation::Receiving { channel: queue_id });
            observe_channel(
                &mut transition,
                observe_full,
                channel,
                ChannelObservation {
                    channel: queue_id,
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
                channel: queue_id,
                task: Some(task),
                kind,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
        let feed = channel.feed;
        if matches!(result, ReceiveResult::Received(_))
            && let Some(hub) = feed
        {
            self.admit_publishers(hub, &mut transition);
        }
        Ok((result, transition))
    }

    pub fn close(
        &mut self,
        task: TaskId,
        completion: Completion,
    ) -> Result<Transition, ChannelError> {
        self.begin_operation(task)?;
        let channel = self
            .channels
            .get(&completion.channel)
            .ok_or(ChannelError::UnknownChannel(completion.channel))?;
        validate_completion(channel, completion, task)?;
        let mut transition = if channel.hub.is_some() {
            self.close_hub(completion.channel, task)
        } else {
            self.close_queue(completion.channel, task)
        };
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    /// Closes the queue `id` on behalf of `task`: parked sends are refused,
    /// and a waiting receiver of an empty queue sees `Drained`.
    fn close_queue(&mut self, id: ChannelId, task: TaskId) -> Transition {
        let observe_boundaries = self.observe_boundaries;
        let channel = self.channels.get_mut(&id).expect("closed queue");
        let mut transition = Transition::default();
        if channel.lifecycle != Lifecycle::Open {
            return transition;
        }
        channel.lifecycle = Lifecycle::Closed;
        transition
            .wakes
            .extend(std::mem::take(&mut channel.select_watchers));
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
                    channel: id,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendRejectedClosed,
                    cause: Some(task),
                    discard_count: 0,
                },
            );
        }
        if channel.queue.is_empty() {
            while let Some(receiver) = channel.waiting_receivers.pop_front() {
                self.operations
                    .insert(receiver, Operation::ReceiveReady(ReceiveResult::Drained));
                transition.wakes.push(receiver);
                observe_channel(
                    &mut transition,
                    observe_boundaries,
                    channel,
                    ChannelObservation {
                        channel: id,
                        task: Some(receiver),
                        kind: ChannelEventKind::Drained,
                        cause: Some(task),
                        discard_count: 0,
                    },
                );
            }
        }
        observe_channel(
            &mut transition,
            observe_boundaries,
            channel,
            ChannelObservation {
                channel: id,
                task: Some(task),
                kind: ChannelEventKind::Closed,
                cause: None,
                discard_count: 0,
            },
        );
        channel.assert_invariants();
        transition
    }

    /// Closes the hub `id`: parked publishes are refused, and every
    /// subscription closes, to drain what it holds.
    fn close_hub(&mut self, id: ChannelId, task: TaskId) -> Transition {
        let observe_boundaries = self.observe_boundaries;
        let hub = self.channels.get_mut(&id).expect("hub");
        let mut transition = Transition::default();
        if hub.lifecycle != Lifecycle::Open {
            return transition;
        }
        hub.lifecycle = Lifecycle::Closed;
        while let Some(waiter) = hub.waiting_senders.pop_front() {
            self.operations.insert(
                waiter.task,
                Operation::SendReady(SendResult::Closed(waiter.value)),
            );
            transition.returned.push(waiter.value);
            transition.wakes.push(waiter.task);
            observe_channel(
                &mut transition,
                observe_boundaries,
                hub,
                ChannelObservation {
                    channel: id,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendRejectedClosed,
                    cause: Some(task),
                    discard_count: 0,
                },
            );
        }
        observe_channel(
            &mut transition,
            observe_boundaries,
            hub,
            ChannelObservation {
                channel: id,
                task: Some(task),
                kind: ChannelEventKind::Closed,
                cause: None,
                discard_count: 0,
            },
        );
        let subscriptions = std::mem::take(&mut hub.hub.as_mut().expect("hub").subscriptions);
        for subscription in subscriptions {
            self.channels.get_mut(&subscription).expect("subscription").feed = None;
            transition.merge(self.close_queue(subscription, task));
        }
        transition
    }

    /// Ends `receiver`, `task`'s current copy: it answers `Aborted` from
    /// now on, and its queue is aborted if no other receiver of it is left.
    pub fn abort(&mut self, task: TaskId, receiver: Receiver) -> Result<Transition, ChannelError> {
        self.begin_operation(task)?;
        let channel = self
            .channels
            .get(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(channel, receiver, task)?;
        let mut transition = self.end_receiver(receiver.channel, Some(task), true);
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    /// Ends the receiver of entry `id`. When it was its queue's last live
    /// receiver, the queue is aborted: always for an explicit abort, and for
    /// an abandoned receiver (owner exit, discarded message, release) while
    /// the queue is open or still holds values. A subscription whose last
    /// receiver ended leaves its hub.
    fn end_receiver(&mut self, id: ChannelId, cause: Option<TaskId>, explicit: bool) -> Transition {
        let mut transition = Transition::default();
        let Some(entry) = self.channels.get_mut(&id) else {
            return transition;
        };
        if entry.receiver_ended {
            return transition;
        }
        entry.receiver_ended = true;
        let queue_id = self.queue_id(id);
        if self.live_receivers(queue_id) > 0 {
            return transition;
        }
        let queue = &self.channels[&queue_id];
        let abandoned_with_work = queue.lifecycle == Lifecycle::Open
            || (queue.lifecycle == Lifecycle::Closed && !queue.queue.is_empty());
        if explicit || abandoned_with_work {
            transition.merge(self.abort_queue(queue_id, cause, ChannelEventKind::Aborted));
        }
        self.leave_hub(queue_id, &mut transition);
        transition
    }

    /// Unlinks the subscription `queue` from its hub, whose parked
    /// publishes may then be accepted.
    fn leave_hub(&mut self, queue: ChannelId, transition: &mut Transition) {
        let Some(hub) = self
            .channels
            .get_mut(&queue)
            .and_then(|channel| channel.feed.take())
        else {
            return;
        };
        if let Some(state) = self.channels.get_mut(&hub).and_then(|hub| hub.hub.as_mut()) {
            state.subscriptions.retain(|subscription| *subscription != queue);
        }
        self.admit_publishers(hub, transition);
    }

    /// Ends a channel the host no longer references. A plain channel is
    /// aborted, which discards its queue and answers its parked peers, and
    /// then forgotten, so any later use of its id is an unknown channel. A
    /// queue that taps still receive from only loses its own receiver, and
    /// is aborted only when nothing can send to it or close it any more; it
    /// is forgotten with its last tap. A hub is aborted while open (nobody
    /// can publish or close it again) and forgotten.
    ///
    /// A release is not one of `task`'s operations: the host issues it for
    /// any task's freed endpoints ahead of `task`'s next request, which may
    /// be the one that collects a result already linearized for `task`.
    /// `None` is a release the carrier drained with no task running.
    pub fn release_unreferenced(
        &mut self,
        task: Option<TaskId>,
        id: ChannelId,
    ) -> Result<Transition, ChannelError> {
        let channel = self
            .channels
            .get(&id)
            .ok_or(ChannelError::UnknownChannel(id))?;
        let mut transition = Transition::default();
        if channel.hub.is_some() {
            transition.merge(self.abort_hub(id, task, ChannelEventKind::Released));
            let hub = self.channels.get_mut(&id).expect("hub");
            if let Some(state) = hub.hub.as_mut() {
                transition.discarded.extend(state.current.take());
                for subscription in std::mem::take(&mut state.subscriptions) {
                    if let Some(subscription) = self.channels.get_mut(&subscription) {
                        subscription.feed = None;
                    }
                }
            }
            self.forget(id);
        } else if let Some(queue) = channel.tap_of {
            transition.merge(self.end_receiver(id, task, false));
            self.forget(id);
            let queue_state = self.channels.get_mut(&queue).expect("tap's queue");
            queue_state.taps.retain(|tap| *tap != id);
            if queue_state.released && queue_state.taps.is_empty() {
                self.forget(queue);
            }
        } else if channel.taps.is_empty() {
            transition.merge(self.abort_queue(id, task, ChannelEventKind::Released));
            self.leave_hub(id, &mut transition);
            self.forget(id);
        } else {
            let unfed = channel.feed.is_none();
            transition.merge(self.end_receiver(id, task, false));
            let open = self.channels[&id].lifecycle == Lifecycle::Open;
            if open && unfed {
                transition.merge(self.abort_queue(id, task, ChannelEventKind::Released));
            }
            self.channels.get_mut(&id).expect("queue").released = true;
        }
        self.settle_parcels(&mut transition);
        Ok(transition)
    }

    /// Removes entry `id` and its owners' index entries.
    fn forget(&mut self, id: ChannelId) {
        let Some(channel) = self.channels.remove(&id) else {
            return;
        };
        for owner in [channel.receiver_owner, channel.completion_owner]
            .into_iter()
            .flatten()
        {
            self.refresh_owner_index(id, owner);
        }
    }

    pub fn transfer_receiver(
        &mut self,
        task: TaskId,
        receiver: Receiver,
        new_owner: TaskId,
    ) -> Result<(Receiver, Transition), ChannelError> {
        self.begin_operation(task)?;
        let observe_boundaries = self.observe_boundaries;
        let queue_id = self.queue_id(receiver.channel);
        if self
            .channels
            .get(&queue_id)
            .is_some_and(|queue| queue.waiting_receivers.contains(&task))
        {
            return Err(ChannelError::CapabilityHasPendingOperation(task));
        }
        let channel = self
            .channels
            .get_mut(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(channel, receiver, task)?;
        self.arrivals += 1;
        channel.receiver_owner = Some(new_owner);
        channel.receiver_since = self.arrivals;
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
        self.arrivals += 1;
        channel.completion_owner = Some(new_owner);
        channel.completion_since = self.arrivals;
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

    /// One receive event of `Select.first`: true when a receive by `task`
    /// would answer at once (a value, a parked rendezvous sender, `Drained`,
    /// or `Aborted`, including an ended receiver). Otherwise `task` becomes
    /// one of the queue's select watchers, woken when that changes.
    /// Consumes nothing either way. The receiver must be `task`'s current
    /// copy, as for a receive.
    pub fn select_receive(
        &mut self,
        task: TaskId,
        receiver: Receiver,
    ) -> Result<bool, ChannelError> {
        self.begin_operation(task)?;
        let entry = self
            .channels
            .get(&receiver.channel)
            .ok_or(ChannelError::UnknownChannel(receiver.channel))?;
        validate_receiver(entry, receiver, task)?;
        if entry.receiver_ended {
            return Ok(true);
        }
        let queue_id = self.queue_id(receiver.channel);
        let channel = self.channels.get_mut(&queue_id).expect("receiver's queue");
        let ready = channel.lifecycle != Lifecycle::Open
            || !channel.queue.is_empty()
            || !channel.waiting_senders.is_empty();
        if !ready && !channel.select_watchers.contains(&task) {
            channel.select_watchers.push(task);
        }
        Ok(ready)
    }

    /// Removes `task` from the select watchers of the queue the receiver
    /// entry `channel` receives from.
    pub fn unwatch(&mut self, task: TaskId, channel: ChannelId) {
        let queue_id = self.queue_id(channel);
        if let Some(channel) = self.channels.get_mut(&queue_id) {
            channel.select_watchers.retain(|watcher| *watcher != task);
        }
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
        if !self.channels.contains_key(&id) {
            return Err(ChannelError::UnknownChannel(id));
        }
        let channel = &self.channels[&self.queue_id(id)];
        Ok(ChannelMetrics {
            capacity: channel.capacity,
            queue_depth: channel.queue.len(),
            waiting_senders: channel.waiting_senders.len(),
            waiting_receiver: !channel.waiting_receivers.is_empty(),
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
            || !channel.waiting_receivers.is_empty()
            || !channel.taps.is_empty()
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

    /// Gives up every value still queued, parked with a sender, held as a
    /// watch's current value, or waiting in a ready result, for a scheduler
    /// that is being dropped.
    pub fn drain_held_values(&mut self) -> Vec<usize> {
        let mut values = Vec::new();
        for id in self.channels.ids_in_order() {
            let channel = self.channels.get_mut(&id).expect("a listed channel");
            values.extend(channel.queue.drain(..));
            values.extend(channel.waiting_senders.drain(..).map(|waiter| waiter.value));
            if let Some(hub) = channel.hub.as_mut() {
                values.extend(hub.current.take());
            }
        }
        for (_, operation) in self.operations.take_all() {
            if let Some(value) = operation.held_value() {
                values.push(value);
            }
        }
        values
    }

    pub fn cleanup_task(&mut self, task: TaskId) -> Transition {
        let mut transition = self.cancel_operation(task);
        self.operations.forget(task);
        let channels = self.owned_channels.remove(&task).unwrap_or_default();
        for id in channels {
            let Some(channel) = self.channels.get(&id) else {
                continue;
            };
            let owns_receiver = channel.receiver_owner == Some(task);
            let abandons_completion =
                channel.completion_owner == Some(task) && channel.lifecycle == Lifecycle::Open;
            if owns_receiver {
                transition.merge(self.end_receiver(id, Some(task), false));
            }
            if abandons_completion {
                transition.merge(self.abort_entry(id, Some(task), ChannelEventKind::Aborted));
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

    /// Advances the arrival clock and returns the new stamp: every
    /// capability a task gains later has a larger one.
    pub fn tick(&mut self) -> u64 {
        self.arrivals += 1;
        self.arrivals
    }

    /// Dates `task`'s capabilities of `channel` that it gained just now to
    /// `since` instead: a child's result handed to its parent counts as
    /// gained when the child started.
    pub fn backdate(&mut self, channel: ChannelId, task: TaskId, since: u64) {
        let Some(channel) = self.channels.get_mut(&channel) else {
            return;
        };
        if channel.receiver_owner == Some(task) && channel.receiver_since > since {
            channel.receiver_since = since;
        }
        if channel.completion_owner == Some(task) && channel.completion_since > since {
            channel.completion_since = since;
        }
    }

    /// Ends the capabilities `task` gained after arrival stamp `since` as its
    /// exit would (`cleanup_task`): a cancelled timeout scope discards the
    /// operation's values, and a capability gained inside it is reachable only
    /// through them (`time-and-outcomes.md`, Semantics of timeouts, step 4).
    pub fn abort_arrivals(&mut self, task: TaskId, since: u64) -> Transition {
        let mut transition = Transition::default();
        let channels: Vec<ChannelId> = self
            .owned_channels
            .get(&task)
            .map(|channels| channels.iter().copied().collect())
            .unwrap_or_default();
        for id in channels {
            let channel = self.channels.get(&id).expect("owned channel");
            let receiver = channel.receiver_owner == Some(task) && channel.receiver_since > since;
            let completion =
                channel.completion_owner == Some(task) && channel.completion_since > since;
            if !receiver && !completion {
                continue;
            }
            let abandons_completion = completion && channel.lifecycle == Lifecycle::Open;
            if receiver {
                transition.merge(self.end_receiver(id, Some(task), false));
            }
            if abandons_completion {
                transition.merge(self.abort_entry(id, Some(task), ChannelEventKind::Aborted));
            }
            let channel = self.channels.get_mut(&id).expect("owned channel");
            if receiver {
                channel.receiver_owner = None;
            }
            if completion {
                channel.completion_owner = None;
            }
            self.refresh_owner_index(id, task);
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
    /// the sender's tokens, or out of the program by ending them. Ending
    /// one can discard or hand back further values, so this runs until the
    /// transition holds no unsettled parcel.
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
        self.arrivals += 1;
        match item.kind {
            CapabilityKind::Receiver => channel.receiver_since = self.arrivals,
            CapabilityKind::Completion => channel.completion_since = self.arrivals,
        }
        let (holder, current, parcel) = channel.holder(item.kind);
        *holder = Some(owner);
        *parcel = None;
        if let Some(token) = token {
            *current = token;
        }
        self.refresh_owner_index(item.channel, owner);
    }

    /// Ends an in-flight capability whose value was discarded, as its
    /// owner's exit would (`cleanup_task`): the receiver ends, and an open
    /// channel's completion aborts it.
    fn release_lost(&mut self, item: Carried) -> Transition {
        let Some(channel) = self.channels.get_mut(&item.channel) else {
            return Transition::default();
        };
        let (_, _, parcel) = channel.holder(item.kind);
        *parcel = None;
        match item.kind {
            CapabilityKind::Receiver => self.end_receiver(item.channel, None, false),
            CapabilityKind::Completion if channel.lifecycle == Lifecycle::Open => {
                self.abort_entry(item.channel, None, ChannelEventKind::Aborted)
            }
            CapabilityKind::Completion => Transition::default(),
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
                channel_state.waiting_receivers.retain(|waiter| *waiter != task);
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

    /// Aborts a whole entry, on behalf of its completion: a hub or a queue.
    fn abort_entry(
        &mut self,
        id: ChannelId,
        cause: Option<TaskId>,
        summary: ChannelEventKind,
    ) -> Transition {
        if self.channels[&id].hub.is_some() {
            self.abort_hub(id, cause, summary)
        } else {
            self.abort_queue(id, cause, summary)
        }
    }

    /// Aborts the hub `id`: parked publishes are refused, a watch's value is
    /// released, and every subscription is aborted.
    fn abort_hub(
        &mut self,
        id: ChannelId,
        cause: Option<TaskId>,
        summary: ChannelEventKind,
    ) -> Transition {
        let observe_boundaries = self.observe_boundaries;
        let hub = self.channels.get_mut(&id).expect("hub");
        let mut transition = Transition::default();
        if hub.lifecycle != Lifecycle::Open {
            return transition;
        }
        hub.lifecycle = Lifecycle::Aborted;
        while let Some(waiter) = hub.waiting_senders.pop_front() {
            self.operations.insert(
                waiter.task,
                Operation::SendReady(SendResult::Aborted(waiter.value)),
            );
            transition.returned.push(waiter.value);
            transition.wakes.push(waiter.task);
            observe_channel(
                &mut transition,
                observe_boundaries,
                hub,
                ChannelObservation {
                    channel: id,
                    task: Some(waiter.task),
                    kind: ChannelEventKind::SendRejectedAborted,
                    cause,
                    discard_count: 0,
                },
            );
        }
        observe_channel(
            &mut transition,
            observe_boundaries,
            hub,
            ChannelObservation {
                channel: id,
                task: cause,
                kind: summary,
                cause,
                discard_count: 0,
            },
        );
        let state = hub.hub.as_mut().expect("hub");
        transition.discarded.extend(state.current.take());
        let subscriptions = std::mem::take(&mut state.subscriptions);
        for subscription in subscriptions {
            self.channels.get_mut(&subscription).expect("subscription").feed = None;
            transition.merge(self.abort_queue(subscription, cause, ChannelEventKind::Aborted));
        }
        transition
    }

    /// Aborts the queue `id` and records `summary` (Aborted or Released) for
    /// it. Aborting an aborted queue again records nothing.
    fn abort_queue(
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
        transition
            .wakes
            .extend(std::mem::take(&mut channel.select_watchers));
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
        while let Some(receiver) = channel.waiting_receivers.pop_front() {
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

/// Hands `value` to the first receiver parked on the queue `channel`, or
/// queues it and wakes the queue's select watchers. A conflating queue that
/// is full drops its unread value first. Requires room, unless conflating.
fn place(
    channel: &mut Channel,
    id: ChannelId,
    operations: &mut Operations,
    value: usize,
    cause: Option<TaskId>,
    observe_full: bool,
    transition: &mut Transition,
) {
    if let Some(receiver) = channel.waiting_receivers.pop_front() {
        operations.insert(
            receiver,
            Operation::ReceiveReady(ReceiveResult::Received(value)),
        );
        transition.delivered.push((value, receiver));
        transition.wakes.push(receiver);
        observe_channel(
            transition,
            observe_full,
            channel,
            ChannelObservation {
                channel: id,
                task: Some(receiver),
                kind: ChannelEventKind::ReceiveDelivered,
                cause,
                discard_count: 0,
            },
        );
        return;
    }
    if channel.conflate && channel.queue.len() >= channel.capacity {
        transition.discarded.extend(channel.queue.pop_front());
    }
    debug_assert!(channel.queue.len() < channel.capacity);
    channel.queue.push_back(value);
    transition
        .wakes
        .extend(std::mem::take(&mut channel.select_watchers));
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
            | Self::SendReady(
                SendResult::Pending | SendResult::Accepted | SendResult::Published(_),
            )
            | Self::Receiving { .. }
            | Self::ReceiveReady(
                ReceiveResult::Pending | ReceiveResult::Drained | ReceiveResult::Aborted,
            ) => None,
        }
    }
}

impl Channel {
    /// The owner, token, and parcel of one of the channel's capabilities.
    fn holder(
        &mut self,
        kind: CapabilityKind,
    ) -> (&mut Option<TaskId>, &mut u64, &mut Option<usize>) {
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
        if !self.waiting_receivers.is_empty() {
            debug_assert!(self.queue.is_empty());
            debug_assert!(self.waiting_senders.is_empty());
        }
        if !self.waiting_senders.is_empty() && self.hub.is_none() {
            debug_assert_eq!(self.queue.len(), self.capacity);
            debug_assert_eq!(self.lifecycle, Lifecycle::Open);
        }
        if self.lifecycle == Lifecycle::Aborted {
            debug_assert!(self.queue.is_empty());
            debug_assert!(self.waiting_senders.is_empty());
            debug_assert!(self.waiting_receivers.is_empty());
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
            waiter_depth: state.waiting_senders.len() + state.waiting_receivers.len(),
            discard_count: event.discard_count,
        },
    );
}

fn record(transition: &mut Transition, enabled: bool, observation: Observation) {
    if enabled {
        transition.observations.push(observation);
    }
}

/// Each task's send or receive, in a slot of its own that stays once made.
///
/// A slot is made by a task's first operation and removed only by
/// `forget`, when the task is cleaned up. Setting or clearing an operation
/// changes only the slot's contents, never the map: the shape the local
/// path of step 4e needs, where a carrier writes the slots of its own
/// tasks while other carriers write theirs (docs/architecture/
/// parallel-scheduler.md, "Step 4 plan", the access model). The shared readers run on the write side only.
#[derive(Default)]
struct Operations {
    slots: crate::scheduler::IdMap<TaskId, std::cell::UnsafeCell<Option<Operation>>>,
}

impl Operations {
    fn insert(&mut self, task: TaskId, operation: Operation) {
        *self.slots.entry(task).or_default().get_mut() = Some(operation);
    }

    fn remove(&mut self, task: &TaskId) -> Option<Operation> {
        self.slots.get_mut(task).and_then(|slot| slot.get_mut().take())
    }

    fn get(&self, task: &TaskId) -> Option<&Operation> {
        // SAFETY: a slot is written through `&mut self`, which cannot coexist
        // with this borrow, or by its task's carrier on the local path
        // (`slot_mut`); these readers run on the write side only.
        self.slots.get(task).and_then(|slot| unsafe { &*slot.get() }.as_ref())
    }

    fn contains_key(&self, task: &TaskId) -> bool {
        self.get(task).is_some()
    }

    /// Every operation, in no particular order.
    fn values(&self) -> impl Iterator<Item = &Operation> {
        // SAFETY: as for `get`.
        self.slots.values().filter_map(|slot| unsafe { &*slot.get() }.as_ref())
    }

    /// Takes every operation, in task order, leaving the slots empty.
    fn take_all(&mut self) -> Vec<(TaskId, Operation)> {
        let mut taken: Vec<_> = self
            .slots
            .iter_mut()
            .filter_map(|(task, slot)| Some((*task, slot.get_mut().take()?)))
            .collect();
        taken.sort_unstable_by_key(|(task, _)| *task);
        taken
    }

    /// `task`'s slot, if it has one.
    ///
    /// # Safety
    ///
    /// As for `Homed::homed_mut`: `task` is homed on the carrier the caller
    /// acts as, and no other reference to this slot is live.
    #[allow(clippy::mut_from_ref)]
    unsafe fn slot_mut(&self, task: TaskId) -> Option<&mut Option<Operation>> {
        // SAFETY: the caller's contract.
        self.slots.get(&task).map(|slot| unsafe { &mut *slot.get() })
    }

    /// Removes `task`'s slot, once it can make no more operations.
    fn forget(&mut self, task: TaskId) {
        self.slots.remove(&task);
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

    // CHI-214: the only Receiver inside a message on its own channel means
    // nobody could ever receive that message.
    #[test]
    fn a_send_carrying_its_own_receiver_is_refused_and_changes_nothing() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (other, _) = channels.create(PARENT, 1);
        assert_eq!(
            channels
                .send_carrying(
                    PARENT,
                    route.sender,
                    MESSAGE,
                    &[carried_receiver(other), carried_receiver(route)]
                )
                .map(|_| ()),
            Err(ChannelError::OwnReceiver(route.sender.channel()))
        );
        assert_eq!(
            channels
                .metrics(route.sender.channel())
                .unwrap()
                .queue_depth,
            0
        );
        let (waiting, _) = channels.receive(PARENT, other.receiver).unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }

    // A closed channel refuses the send and hands the receiver back, as it
    // always did, so there is nothing to defect over.
    #[test]
    fn a_send_carrying_its_own_receiver_on_a_closed_channel_is_refused_as_closed() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        channels.close(PARENT, route.completion).unwrap();
        let (result, _) = channels
            .send_carrying(PARENT, route.sender, MESSAGE, &[carried_receiver(route)])
            .unwrap();
        assert_eq!(result, SendResult::Closed(MESSAGE));
    }

    // A receiver whose task ended reads nothing, so it does not make the
    // message receivable.
    #[test]
    fn a_send_carrying_its_receiver_is_refused_when_the_other_receiver_ended() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (tap, _) = channels.add_receiver(PARENT, route.receiver).unwrap();
        channels.transfer_receiver(PARENT, tap, CHILD).unwrap();
        channels.cleanup_task(CHILD);
        assert_eq!(
            channels
                .send_carrying(PARENT, route.sender, MESSAGE, &[carried_receiver(route)])
                .map(|_| ()),
            Err(ChannelError::OwnReceiver(route.sender.channel()))
        );
    }

    // With a second receiver of the same queue, the message can still be
    // received, so the send goes through.
    #[test]
    fn a_send_carrying_one_of_two_receivers_of_its_queue_is_accepted() {
        let mut channels = registry();
        let (route, _) = channels.create(PARENT, 1);
        let (tap, _) = channels.add_receiver(PARENT, route.receiver).unwrap();
        let tap = Carried {
            channel: tap.channel(),
            kind: CapabilityKind::Receiver,
            token: tap.token(),
        };
        assert_eq!(
            channels
                .send_carrying(PARENT, route.sender, MESSAGE, &[carried_receiver(route), tap])
                .map(|_| ()),
            Err(ChannelError::OwnReceiver(route.sender.channel()))
        );
        let (result, _) = channels
            .send_carrying(PARENT, route.sender, MESSAGE, &[carried_receiver(route)])
            .unwrap();
        assert_eq!(result, SendResult::Accepted);
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
        assert_eq!(
            channels
                .metrics(route.sender.channel())
                .unwrap()
                .queue_depth,
            0
        );
        let (waiting, _) = channels.receive(CHILD, carried.receiver).unwrap();
        assert_eq!(waiting, ReceiveResult::Pending);
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    const OWNER: TaskId = TaskId::from_raw(1);
    const WORKER: TaskId = TaskId::from_raw(2);
    const OTHER: TaskId = TaskId::from_raw(3);

    fn registry() -> ChannelRegistry {
        ChannelRegistry::new(true, true)
    }

    /// A registry whose copies are distinguishable: a copy of `v` is
    /// `v * 1000 + n` for the n-th copy made.
    fn copying_registry() -> ChannelRegistry {
        let mut channels = registry();
        let mut made = 0;
        channels.set_copier(Box::new(move |value| {
            made += 1;
            value * 1000 + made
        }));
        channels
    }

    fn receive(channels: &mut ChannelRegistry, task: TaskId, receiver: Receiver) -> ReceiveResult {
        let (result, _) = channels.receive(task, receiver).expect("a valid receive");
        if result != ReceiveResult::Pending {
            channels.take_receive_result(task);
        }
        result
    }

    fn send(channels: &mut ChannelRegistry, task: TaskId, sender: Sender, value: usize) -> SendResult {
        let (result, _) = channels
            .send_carrying(task, sender, value, &[])
            .expect("a valid send");
        if result != SendResult::Pending {
            channels.take_send_result(task);
        }
        result
    }

    #[test]
    fn receivers_of_one_queue_each_take_a_different_value_in_parking_order() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 0);
        let (added, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        let (second, _) = channels.transfer_receiver(OWNER, added, WORKER).unwrap();
        assert_eq!(receive(&mut channels, WORKER, second), ReceiveResult::Pending);
        assert_eq!(receive(&mut channels, OWNER, queue.receiver), ReceiveResult::Pending);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 7), SendResult::Accepted);
        assert_eq!(
            channels.take_receive_result(WORKER),
            Some(ReceiveResult::Received(7))
        );
        assert_eq!(channels.take_receive_result(OWNER), None);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 8), SendResult::Accepted);
        assert_eq!(
            channels.take_receive_result(OWNER),
            Some(ReceiveResult::Received(8))
        );
    }

    #[test]
    fn aborting_one_receiver_leaves_the_queue_to_the_others_until_the_last_ends() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 2);
        let (second, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        channels.abort(OWNER, queue.receiver).unwrap();
        assert_eq!(receive(&mut channels, OWNER, queue.receiver), ReceiveResult::Aborted);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 1), SendResult::Accepted);
        assert_eq!(receive(&mut channels, OWNER, second), ReceiveResult::Received(1));
        channels.abort(OWNER, second).unwrap();
        assert_eq!(send(&mut channels, OTHER, queue.sender, 2), SendResult::Aborted(2));
    }

    #[test]
    fn a_receiver_added_through_an_ended_one_is_ended() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 1);
        let (second, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        channels.abort(OWNER, queue.receiver).unwrap();
        let (third, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        assert_eq!(receive(&mut channels, OWNER, third), ReceiveResult::Aborted);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 1), SendResult::Accepted);
        assert_eq!(receive(&mut channels, OWNER, second), ReceiveResult::Received(1));
    }

    #[test]
    fn a_worker_exit_ends_only_its_receiver() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 1);
        let (mine, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        let (moved, _) = channels.transfer_receiver(OWNER, mine, WORKER).unwrap();
        assert_eq!(receive(&mut channels, WORKER, moved), ReceiveResult::Pending);
        channels.cleanup_task(WORKER);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 5), SendResult::Accepted);
        assert_eq!(receive(&mut channels, OWNER, queue.receiver), ReceiveResult::Received(5));
        channels.cleanup_task(OWNER);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 6), SendResult::Aborted(6));
    }

    #[test]
    fn a_receiver_in_flight_keeps_its_queue_open_after_the_sender_exits() {
        let mut channels = registry();
        let (route, _) = channels.create(WORKER, 1);
        let (queue, _) = channels.create(OWNER, 1);
        let (added, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        let carried = Carried {
            channel: added.channel(),
            kind: CapabilityKind::Receiver,
            token: added.token(),
        };
        let (sent, _) = channels
            .send_carrying(OWNER, route.sender, 99, &[carried])
            .unwrap();
        assert_eq!(sent, SendResult::Accepted);
        channels.take_send_result(OWNER);
        channels.transfer_completion(OWNER, queue.completion, OTHER).unwrap();
        channels.cleanup_task(OWNER);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 1), SendResult::Accepted);
        assert_eq!(receive(&mut channels, WORKER, route.receiver), ReceiveResult::Received(99));
        let landed = Receiver::from_raw(added.channel().get(), WORKER.get(), added.token() + 1);
        assert_eq!(receive(&mut channels, WORKER, landed), ReceiveResult::Received(1));
    }

    #[test]
    fn select_on_a_shared_queue_wakes_every_watcher() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 1);
        let (added, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        let (second, _) = channels.transfer_receiver(OWNER, added, WORKER).unwrap();
        assert_eq!(channels.select_receive(OWNER, queue.receiver), Ok(false));
        assert_eq!(channels.select_receive(WORKER, second), Ok(false));
        let (_, transition) = channels.send_carrying(OTHER, queue.sender, 3, &[]).unwrap();
        assert!(transition.wakes.contains(&OWNER));
        assert!(transition.wakes.contains(&WORKER));
    }

    #[test]
    fn select_on_an_ended_receiver_is_ready() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 1);
        let (second, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        channels.abort(OWNER, second).unwrap();
        assert_eq!(channels.select_receive(OWNER, second), Ok(true));
        assert_eq!(channels.select_receive(OWNER, queue.receiver), Ok(false));
    }

    #[test]
    fn a_released_queue_still_drains_to_its_taps_once_closed() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 2);
        let (tap, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        send(&mut channels, OWNER, queue.sender, 4);
        channels.close(OWNER, queue.completion).unwrap();
        channels.release_unreferenced(Some(OWNER), queue.sender.channel()).unwrap();
        assert_eq!(receive(&mut channels, OWNER, tap), ReceiveResult::Received(4));
        assert_eq!(receive(&mut channels, OWNER, tap), ReceiveResult::Drained);
        channels.release_unreferenced(Some(OWNER), tap.channel()).unwrap();
        assert_eq!(channels.len(), 0);
    }

    #[test]
    fn a_released_open_queue_aborts_its_taps() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 2);
        let (tap, _) = channels.add_receiver(OWNER, queue.receiver).unwrap_err_none();
        channels.release_unreferenced(Some(OWNER), queue.sender.channel()).unwrap();
        assert_eq!(receive(&mut channels, OWNER, tap), ReceiveResult::Aborted);
    }

    #[test]
    fn a_release_waits_for_no_uncollected_result() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 0);
        let (other, _) = channels.create(OWNER, 1);
        assert_eq!(receive(&mut channels, OWNER, queue.receiver), ReceiveResult::Pending);
        assert_eq!(send(&mut channels, OTHER, queue.sender, 7), SendResult::Accepted);
        channels.release_unreferenced(Some(OWNER), other.sender.channel()).unwrap();
        assert_eq!(
            channels.take_receive_result(OWNER),
            Some(ReceiveResult::Received(7))
        );
    }

    fn broadcast(channels: &mut ChannelRegistry, capacity: usize) -> (Sender, Completion) {
        let (publisher, completion, _) = channels.create_hub(OWNER, HubKind::Broadcast, capacity, None);
        (publisher, completion)
    }

    #[test]
    fn every_subscription_gets_its_own_copy_of_each_publish() {
        let mut channels = copying_registry();
        let (publisher, _) = broadcast(&mut channels, 2);
        let (first, _) = channels.subscribe(WORKER, publisher).unwrap();
        let (second, _) = channels.subscribe(OTHER, publisher).unwrap();
        assert_eq!(send(&mut channels, OWNER, publisher, 5), SendResult::Published(2));
        let one = receive(&mut channels, WORKER, first);
        let two = receive(&mut channels, OTHER, second);
        let ReceiveResult::Received(one) = one else { panic!("{one:?}") };
        let ReceiveResult::Received(two) = two else { panic!("{two:?}") };
        assert_ne!(one, two);
        assert!([one, two].contains(&5));
        assert!([one, two].contains(&5001));
    }

    #[test]
    fn a_subscription_sees_only_publishes_accepted_after_it_subscribed() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 2);
        let (early, _) = channels.subscribe(WORKER, publisher).unwrap();
        send(&mut channels, OWNER, publisher, 1);
        let (late, _) = channels.subscribe(OTHER, publisher).unwrap();
        send(&mut channels, OWNER, publisher, 2);
        assert_eq!(receive(&mut channels, WORKER, early), ReceiveResult::Received(1));
        assert_eq!(receive(&mut channels, WORKER, early), ReceiveResult::Received(2));
        assert_eq!(receive(&mut channels, OTHER, late), ReceiveResult::Received(2));
        assert_eq!(receive(&mut channels, OTHER, late), ReceiveResult::Pending);
    }

    #[test]
    fn a_publish_with_no_subscriptions_reaches_none_and_releases_its_value() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (result, transition) = channels.send_carrying(OWNER, publisher, 9, &[]).unwrap();
        assert_eq!(result, SendResult::Published(0));
        assert_eq!(transition.discarded, vec![9]);
    }

    #[test]
    fn a_full_subscription_parks_the_publish_until_it_receives() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (slow, _) = channels.subscribe(WORKER, publisher).unwrap();
        let (fast, _) = channels.subscribe(OTHER, publisher).unwrap();
        assert_eq!(send(&mut channels, OWNER, publisher, 1), SendResult::Published(2));
        assert_eq!(receive(&mut channels, OTHER, fast), ReceiveResult::Received(1));
        assert_eq!(send(&mut channels, OWNER, publisher, 2), SendResult::Pending);
        assert_eq!(receive(&mut channels, OTHER, fast), ReceiveResult::Pending);
        assert_eq!(receive(&mut channels, WORKER, slow), ReceiveResult::Received(1));
        assert_eq!(
            channels.take_send_result(OWNER),
            Some(SendResult::Published(2))
        );
        assert_eq!(
            channels.take_receive_result(OTHER),
            Some(ReceiveResult::Received(2))
        );
    }

    #[test]
    fn a_full_subscription_that_ends_lets_the_parked_publish_through() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (slow, _) = channels.subscribe(WORKER, publisher).unwrap();
        let (fast, _) = channels.subscribe(OTHER, publisher).unwrap();
        send(&mut channels, OWNER, publisher, 1);
        receive(&mut channels, OTHER, fast);
        assert_eq!(send(&mut channels, OWNER, publisher, 2), SendResult::Pending);
        channels.abort(WORKER, slow).unwrap();
        assert_eq!(
            channels.take_send_result(OWNER),
            Some(SendResult::Published(1))
        );
        assert_eq!(receive(&mut channels, OTHER, fast), ReceiveResult::Received(2));
    }

    #[test]
    fn a_subscription_made_while_a_publish_is_parked_receives_it() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (slow, _) = channels.subscribe(WORKER, publisher).unwrap();
        send(&mut channels, OWNER, publisher, 1);
        assert_eq!(send(&mut channels, OWNER, publisher, 2), SendResult::Pending);
        let (late, _) = channels.subscribe(OTHER, publisher).unwrap();
        receive(&mut channels, WORKER, slow);
        assert_eq!(
            channels.take_send_result(OWNER),
            Some(SendResult::Published(2))
        );
        assert_eq!(receive(&mut channels, OTHER, late), ReceiveResult::Received(2));
    }

    #[test]
    fn a_cancelled_parked_publish_delivers_nothing() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (slow, _) = channels.subscribe(WORKER, publisher).unwrap();
        send(&mut channels, OWNER, publisher, 1);
        assert_eq!(send(&mut channels, OWNER, publisher, 2), SendResult::Pending);
        let transition = channels.cancel_pending(OWNER);
        assert_eq!(transition.discarded, vec![2]);
        assert_eq!(receive(&mut channels, WORKER, slow), ReceiveResult::Received(1));
        assert_eq!(receive(&mut channels, WORKER, slow), ReceiveResult::Pending);
    }

    #[test]
    fn closing_a_broadcast_refuses_parked_publishes_and_drains_each_subscription() {
        let mut channels = registry();
        let (publisher, completion) = broadcast(&mut channels, 1);
        let (sub, _) = channels.subscribe(WORKER, publisher).unwrap();
        send(&mut channels, OTHER, publisher, 1);
        assert_eq!(send(&mut channels, OTHER, publisher, 2), SendResult::Pending);
        channels.close(OWNER, completion).unwrap();
        assert_eq!(channels.take_send_result(OTHER), Some(SendResult::Closed(2)));
        assert_eq!(send(&mut channels, OTHER, publisher, 3), SendResult::Closed(3));
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Received(1));
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Drained);
        let (after, _) = channels.subscribe(WORKER, publisher).unwrap();
        assert_eq!(receive(&mut channels, WORKER, after), ReceiveResult::Drained);
    }

    #[test]
    fn an_abandoned_broadcast_completion_aborts_every_subscription() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (sub, _) = channels.subscribe(WORKER, publisher).unwrap();
        send(&mut channels, OTHER, publisher, 1);
        assert_eq!(send(&mut channels, OTHER, publisher, 2), SendResult::Pending);
        channels.cleanup_task(OWNER);
        assert_eq!(channels.take_send_result(OTHER), Some(SendResult::Aborted(2)));
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Aborted);
        let (after, _) = channels.subscribe(WORKER, publisher).unwrap();
        assert_eq!(receive(&mut channels, WORKER, after), ReceiveResult::Aborted);
    }

    #[test]
    fn a_published_value_cannot_carry_a_capability() {
        let mut channels = registry();
        let (publisher, _) = broadcast(&mut channels, 1);
        let (queue, _) = channels.create(OWNER, 1);
        let carried = Carried {
            channel: queue.receiver.channel(),
            kind: CapabilityKind::Receiver,
            token: 1,
        };
        assert_eq!(
            channels.send_carrying(OWNER, publisher, 1, &[carried]).map(|(result, _)| result),
            Err(ChannelError::PublishedCapability(publisher.channel()))
        );
        assert_eq!(receive(&mut channels, OWNER, queue.receiver), ReceiveResult::Pending);
    }

    #[test]
    fn subscribing_to_a_plain_channel_is_the_wrong_kind() {
        let mut channels = registry();
        let (queue, _) = channels.create(OWNER, 1);
        assert_eq!(
            channels.subscribe(OWNER, queue.sender).map(|(receiver, _)| receiver.channel()),
            Err(ChannelError::WrongKind(queue.sender.channel()))
        );
    }

    fn watch(channels: &mut ChannelRegistry, initial: usize) -> (Sender, Completion) {
        let (publisher, completion, _) = channels.create_hub(OWNER, HubKind::Watch, 1, Some(initial));
        (publisher, completion)
    }

    #[test]
    fn a_watch_subscription_starts_with_the_current_value_and_then_waits() {
        let mut channels = copying_registry();
        let (publisher, _) = watch(&mut channels, 10);
        let (sub, _) = channels.subscribe(WORKER, publisher).unwrap();
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Received(10_001));
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Pending);
        assert_eq!(send(&mut channels, OWNER, publisher, 11), SendResult::Accepted);
        assert_eq!(
            channels.take_receive_result(WORKER),
            Some(ReceiveResult::Received(11_002))
        );
    }

    #[test]
    fn a_watch_publish_never_parks_and_replaces_an_unread_value() {
        let mut channels = registry();
        let (publisher, _) = watch(&mut channels, 1);
        let (sub, _) = channels.subscribe(WORKER, publisher).unwrap();
        let (_, transition) = channels.send_carrying(OWNER, publisher, 2, &[]).unwrap();
        channels.take_send_result(OWNER);
        assert!(transition.discarded.contains(&1));
        assert_eq!(send(&mut channels, OWNER, publisher, 3), SendResult::Accepted);
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Received(3));
        assert_eq!(receive(&mut channels, WORKER, sub), ReceiveResult::Pending);
    }

    #[test]
    fn a_closed_watch_gives_its_unseen_final_value_then_drained() {
        let mut channels = registry();
        let (publisher, completion) = watch(&mut channels, 1);
        let (seen, _) = channels.subscribe(WORKER, publisher).unwrap();
        receive(&mut channels, WORKER, seen);
        send(&mut channels, OWNER, publisher, 2);
        channels.close(OWNER, completion).unwrap();
        assert_eq!(send(&mut channels, OWNER, publisher, 3), SendResult::Closed(3));
        assert_eq!(receive(&mut channels, WORKER, seen), ReceiveResult::Received(2));
        assert_eq!(receive(&mut channels, WORKER, seen), ReceiveResult::Drained);
        let (late, _) = channels.subscribe(OTHER, publisher).unwrap();
        assert_eq!(receive(&mut channels, OTHER, late), ReceiveResult::Received(2));
        assert_eq!(receive(&mut channels, OTHER, late), ReceiveResult::Drained);
    }

    #[test]
    fn releasing_a_watch_releases_its_current_value() {
        let mut channels = registry();
        let (publisher, _) = watch(&mut channels, 7);
        let transition = channels.release_unreferenced(Some(OWNER), publisher.channel()).unwrap();
        assert_eq!(transition.discarded, vec![7]);
        assert_eq!(channels.len(), 0);
    }

    trait AddedReceiver {
        fn unwrap_err_none(self) -> (Receiver, Transition);
    }

    impl AddedReceiver for Result<(Receiver, Transition), ChannelError> {
        fn unwrap_err_none(self) -> (Receiver, Transition) {
            self.expect("a valid add_receiver")
        }
    }
}
