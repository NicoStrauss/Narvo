# ADR-0011: An event sent in a tick is readable in the next one, and the buffer is in the hash

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`Events`,
`rotate_events`), and every system that sends or reads one

## Context

Systems are `fn` pointers and cannot capture state (ADR-0002's facade, enforced
by the `System` signature), so anything a system wants to tell another system
has to travel through the world. That much was already settled. What was not
settled is *when* the message becomes visible, and that question has two
defensible answers whose difference only shows up as a bug months later.

**Same tick.** A system sends; systems scheduled after it in the same tick see
the event. This is what most engines do and it has the lower latency.

**Next tick.** A system sends; nothing sees it until the following tick, when
every system sees it.

Whichever is chosen has to be chosen, written down, and enforced by one
mechanism. A system that half-does either — delivering within the tick when the
reader happens to be scheduled later, and a tick late when it happens to be
scheduled earlier — is the origin of nondeterminism that cannot be found
afterwards, because nothing about it is visibly wrong at any single point.

## Decision

**An event sent during tick *N* is readable by every system during tick *N+1*,
by none during tick *N*, and by none during tick *N+2*.**

Concretely:

- `Events<E>` is a component holding two buffers: what has been sent this tick,
  and what is readable this tick.
- `rotate_events::<E>` is an ordinary system, registered by the application,
  normally first in the run order. It moves one to the other and drops what was
  readable before.
- `iter` yields events in the order they were sent. The buffers are `Vec`s;
  nothing sorts, groups or deduplicates, and no hash map is consulted anywhere
  between `send` and `iter`.
- **The buffer is registered and therefore part of the canonical dump and the
  state hash.**

## Rationale

1. **Next-tick delivery makes visibility independent of the schedule.** Under
   same-tick delivery, whether a system sees an event depends on where it sits
   relative to the sender. Moving a system for an unrelated reason — a new
   dependency, a rename, a tidy-up — then changes behaviour silently: no error,
   no failed assertion, just an event that now arrives a tick later, or never.
   The engine cannot detect it, because both orders are legal. Under next-tick
   delivery every system in a tick sees exactly the same set of events whatever
   its position, which is a property that fits in one sentence and in one test.

2. **The cost is known and small.** One tick of latency, under 17 ms at 60 Hz.
   The case that is genuinely hurt — a reaction that cannot wait a tick — is
   better served by the system that caused it doing the work directly than by a
   message, and the case that is helped is every future refactor of the run
   order.

3. **A pending event is simulation state, so it belongs in the hash.** Two runs
   holding different pending events have already diverged, even though every
   other component still matches; leaving the buffer out of the dump would hide
   precisely the tick on which the divergence began — the one the instrument
   exists to catch. This is ADR-0010's argument for the generator's state,
   applied unchanged.

   The alternative — keeping the hash focused on "real" simulation state and
   treating buffers as transient — was considered and rejected. "Transient"
   here means "decides what happens next tick", which is not transient in any
   sense the hash cares about. It would also have needed new machinery:
   `canonical_dump` refuses to serialize a world containing an unregistered
   component, so excluding the buffer means either storing it outside the world,
   which breaks the rule that systems own no state, or building an opt-out list
   whose only purpose is to hide state from the comparison. Adding a mechanism
   for *not looking* at part of the state is the wrong direction for this
   project.

4. **In the hash, a forgotten rotation is loud instead of silent.** If nobody
   registers `rotate_events::<E>`, events pile up unread. That is a mistake the
   type system cannot catch — but with the buffer in the dump, the pile is in
   the dump, growing, visible in the first diff anybody looks at. Outside the
   dump it would be an event system that silently never delivers.

5. **Rotation is a registered system rather than something the scheduler does.**
   The scheduler knows nothing about event types and should not learn; more
   importantly, a rotation that happened invisibly would be a tick boundary that
   does not appear in the run order. Registered, it shows up in
   `Scheduler::system_names` beside everything else, and where it sits is a
   decision the application makes and a reader can see.

## Consequences

- **An event type has to be serializable**, like any other component that
  reaches the dump. That is the same requirement CLAUDE.md already places on
  components and not a new burden.
- **Registering the rotation is the application's job**, and forgetting it is
  possible. Mitigated as described above rather than prevented; a mechanism that
  prevented it would have to know every event type in the world, which is the
  reflection this engine deliberately does not have.
- **An event is delivered exactly once and expires.** A system that does not
  look during the one tick an event is readable does not get another chance.
  Anything needing durable state should write a component, which is what
  components are for.
- **No request/response within a tick.** A system that sends and expects an
  answer gets it two ticks later at the earliest. That is a real limitation and
  the one thing given up by this decision.
- **Several buffers of the same type are rotated in query order, which is not
  stable.** Harmless here and only here: rotating a buffer touches nothing
  outside it, so any visitation order produces the same result. It is written
  down because "unstable order that happens not to matter" is exactly the kind
  of thing that stops not mattering after a later change.
- The serialized form of a buffer — both halves, labelled — is observable
  surface. Changing it changes every state hash of every simulation holding one.

## Revision condition

Reopen if a milestone needs a genuine within-tick channel — physics contact
events feeding a response in the same step (M5) is the plausible one. That would
be a *second*, separately named mechanism with its own ordering rules, not a
loosening of this one: the failure mode in the Context section is caused by
having one channel with two behaviours, and it comes back the moment this one
grows a same-tick mode.

Also reopen if the one-tick delay is ever measured to be a problem rather than
suspected to be one.
