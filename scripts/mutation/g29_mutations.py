"""Mutants of G29 (CHI-231), the channel variants: several receivers of one
queue, std/broadcast and std/watch, and the checker rules that came with them
(CHI-224). See docs/specification/task-and-channel-capabilities.md, "Channel
variants".

Run: scripts/mutation/mutate.py scripts/mutation/g29_mutations.py \\
         scripts/mutation/g29_fiber_mutations.py
Results: scripts/mutation/g29_results.txt.
"""

CRATE = "compiler/chimi"

CHANNEL = "../../runtime/chimi-fiber/src/channel.rs"
RUNTIME_C = "runtime/native/runtime.c"
CAPABILITY = "src/capability.rs"


def variant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["channel_variants"])


MUTATIONS = [
    # Several receivers.
    variant("an added receiver reads its own empty entry", CHANNEL, [(
        "            .and_then(|channel| channel.tap_of)\n            .unwrap_or(id)",
        "            .and_then(|_| None::<ChannelId>)\n            .unwrap_or(id)")]),
    variant("added receivers do not keep the queue alive", CHANNEL, [(
        "        usize::from(!channel.receiver_ended)\n            + channel",
        "        usize::from(!channel.receiver_ended)\n            + 0 * channel")]),
    variant("ending any receiver aborts the queue", CHANNEL, [(
        "        if self.live_receivers(queue_id) > 0 {\n            return transition;\n        }\n",
        "")]),
    variant("a receiver added through an ended one is live", CHANNEL, [(
        "let ended = ended || queue.lifecycle == Lifecycle::Aborted;",
        "let ended = queue.lifecycle == Lifecycle::Aborted;")]),
    # Broadcast.
    variant("a publish waits only for some subscription to have room", CHANNEL, [(
        "            .all(|subscription| self.channels[subscription].has_room())",
        "            .any(|subscription| self.channels[subscription].has_room())")]),
    variant("a publish counts at most one subscription", CHANNEL, [(
        "        self.place_into(*last, value, Some(cause), transition);\n        subscriptions.len()",
        "        self.place_into(*last, value, Some(cause), transition);\n        subscriptions.len().min(1)")]),
    variant("a publish with no subscriptions claims one", CHANNEL, [(
        "            transition.discarded.push(value);\n            return 0;",
        "            transition.discarded.push(value);\n            return 1;")]),
    variant("a subscription made after the close is open", CHANNEL, [(
        "        channel.lifecycle = lifecycle;\n        channel.conflate",
        "        channel.lifecycle = Lifecycle::Open;\n        channel.conflate")]),
    variant("closing a broadcast leaves its subscriptions open", CHANNEL, [(
        "            transition.merge(self.close_queue(subscription, task));\n",
        "")]),
    variant("a receive does not admit a parked publish", CHANNEL, [(
        "            self.admit_publishers(hub, &mut transition);\n        }\n        Ok((result, transition))",
        "        }\n        Ok((result, transition))")]),
    variant("publish returns no count", RUNTIME_C, [(
        "? integer_value(request.value)\n                           : void_value());",
        "? integer_value(0)\n                           : void_value());")]),
    variant("a zero capacity broadcast is not rejected by name", RUNTIME_C, [(
        "      if (arguments[0]->as.integer < 1)\n        return defect(numeric_message(\"`Broadcast.bounded",
        "      if (arguments[0]->as.integer < 0)\n        return defect(numeric_message(\"`Broadcast.bounded")]),
    # Watch.
    variant("a new watch subscription has not got the current value", CHANNEL, [(
        "            let copy = self.copy_value(current);\n            channel.queue.push_back(copy);\n",
        "")]),
    variant("a watch publish keeps the old current value", CHANNEL, [(
        "transition.discarded.extend(state.current.replace(value));",
        "transition.discarded.push(value);")]),
    # Checker.
    variant("add_receiver moves its argument", CAPABILITY, [(
        "                    && standard != Some(StandardFunction::ChannelAddReceiver);",
        ";")]),
    variant("an exclusive value can be published", CAPABILITY, [(
        "        if value.properties.shareable != PropertyDerivation::Never {\n            return;",
        "        if true {\n            return;")]),
    variant("an event does not borrow its receiver", CAPABILITY, [(
        "                    result.borrows.extend(receiver.carries.iter().copied());\n",
        "")]),
    variant("a read never checks what its value borrows", CAPABILITY, [(
        "                    self.check_borrows(module_id, name, &binding.value, environment);\n",
        "")]),
    variant("every call result keeps its arguments' borrows", CAPABILITY, [(
        "                    if embeds {\n                        if holds_carried {",
        "                    result.borrows.extend(value.borrows.iter().copied());\n                    if embeds {\n                        if holds_carried {")]),
    variant("an event read through a field borrows nothing", CAPABILITY, [(
        "                        .extend(self.field_root_binding(module_id, argument, environment));",
        "                        .extend(None::<SymbolId>);")]),
    variant("a move within the task counts as leaving it", CAPABILITY, [(
        "            } => current = owner,",
        "            } => return Some(TransferBoundary::IntoBinding(owner)),")]),
]
