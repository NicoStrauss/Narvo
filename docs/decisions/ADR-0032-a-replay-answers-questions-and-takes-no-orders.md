# ADR-0032: A replay answers questions and takes no orders

Status: accepted · Date: 2026-08-13 · Scope: `narvo-app`'s `ipc` module and its
headless runner, `narvo-ipc`'s vocabulary, and every command the agent protocol
gains from here on

## Context

M6.4a gave the protocol its fourth and fifth commands — `load_scene`, which
constitutes the running world afresh from a scene file (ADR-0022's
reconstitution), and `replay`, which makes the run a reproduction of a recording.
Neither is an architectural decision on its own: the first goes through
`sim::scene_file::build`, which already had two callers, and the second through
`headless::begin`, which is the runner's own prologue extracted so that a
socket-started replay and a `--replay` command line cannot disagree.

What *is* a decision is what those two do to a run that is **already** a replay,
and the question could not be avoided because the answer for one of them was
already shipped and wrong.

**M6.3c refused a `step` during a replay**, out of `RunError::TooShort`'s own
reasoning: past the end of a recording a run continues with no input at all and
reproduces nothing. It refused nothing else, and the reason was not that the
others had been considered — it was that no other command that steers a run
existed yet.

**A write during a replay was reachable and was accepted.** `--replay` and
`--ipc` are not mutually exclusive on the command line (`cli.rs:552-593` lists
the flags `--replay` refuses and `--ipc` is not among them), so an agent could
attach to a replay and write to its world. M6.4a drove it before deciding
anything, against `48987c0`'s binary:

```
$ narvo --headless --mode input --ticks 20 --record probe.rec --hash
1445489f4731792a

$ narvo --headless --replay probe.rec --ipc 127.0.0.1:0 --hash
step  -> {"error":{"message":"step is refused during a replay: …"}}
set   -> {"set_component":{"entity":"0v1","component":"position","previous":"(x:0,y:14)"}}
get   -> {"get_component":{"entity":"0v1","component":"position","value":"(x:9999,y:-9999)"}}
1fd7d72ac15a9712
```

A replay reported a state hash that is **not** the recorded run's, and the
recording it produced was byte-identical to the one it had been given — the run's
own account said "this is that recording" about a world that had been written to.

D19's band cut cannot report it, and that is structural rather than an oversight.
The cut sets the band's tick count to the ticks that have run; during a
full-length replay the band already covers exactly that many, so `cut_to` is a
no-op, and `Recording::cut_to`'s own documentation records why nothing else could
carry the fact: only ticks with input are stored (ADR-0012 Decision 2), so a band
that stops early is byte-indistinguishable from a run that simply had nothing to
record. There is no marker and a `cut` header field would be an unverifiable
claim.

So a rule was needed that covers `set` as well as `step`, and — because
`load_scene` and `replay` arrive in the same task — one that will still answer
for the commands that come after them.

## Decision

**A run that is reproducing a recording answers reads and refuses every command
that would change what it reproduces.**

The criterion is that sentence and nothing finer. Applied to the five commands
there are:

| command | during a replay | why |
|---|---|---|
| `list_entities` | **answered** | reading a replayed world changes nothing about what it reproduces |
| `get_entity` | **answered** | the same |
| `get_component` | **answered** | the same |
| `set_component` | refused | the world stops being one the recording describes |
| `step` | refused | a replay's length is its recording's (M6.3c) |
| `load_scene` | refused | a world from another file is not the one the recording was made against (ADR-0019) |
| `replay` | refused | a second recording abandons the first part-way |

It is **one** check, `refuse_while_replaying` (`crates/narvo-app/src/ipc.rs`),
taking the command's name and its consequence, and one message shape:

```
<command> is refused during a replay: a replay reproduces the run its recording
describes, and <consequence>. A replay answers questions and takes no orders —
let it finish, or start a live run to steer
```

Four handlers call it as their first statement. M6.3c's sentence survives word
for word as `step`'s consequence clause, so the widening cost no wording.

**"Reproducing" is asked of the run's input source, not remembered from its
plan.** `Source::Recorded { .. }` is the whole of the test
(`Stage::replaying`). Until M6.4a `run_with` read it once before tick 0 into a
`let replaying`, which was correct while only a command line could start a
replay; a `replay` request makes it a property of the moment, and a run that is
live at tick 4 and reproducing at tick 5 has to be refused accordingly.

## Rejected alternatives

**Allow the write and cut the band.** The strongest of the three, because it is
what D19 already prescribes for a live run and because "replay to tick 500, then
poke a value and see what happens" is a real debugging wish. Rejected on the
measurement above: during a replay the cut is a no-op, so the one instrument that
would record that the run stopped being a reproduction records nothing, and the
result is a hash that looks like a reproduction's and is not. A rule whose
enforcement mechanism is inert in exactly the case it is invoked for is not a
rule.

**Allow it and give the recording a mutation marker.** This is the shape D19
itself rejected in M6.2 for the live case, and ADR-0012 Decision 2 is why: the
format stores only ticks that carry input, so a marker would be a new kind of
line whose absence proves nothing. It would also make a replay's output band
differ from its input band, which is the property
`a_replay_produces_the_recording_it_was_given` rests on.

**Refuse only `set` and `step`, and let `load_scene` and `replay` through.**
Defensible one command at a time — a scene load during a replay "merely" ends the
reproduction rather than corrupting it — and rejected because it is four separate
answers to one question, which is how they come to disagree. It also has a
concrete failure: a scene load leaves the run's remaining recorded inputs pointed
at a world whose entity slots mean something else, and `Source::take` matches
tick numbers exactly, so they would be delivered silently into the wrong world.

**Refuse everything, reads included.** Rejected because it takes away the one
thing an agent attached to a replay is for. Reading is how a divergence is
localised, and ADR-0008 already makes the state hash the instrument for "did the
two runs compute the same thing" — being able to ask *what* differs is the
capability M6 exists to add.

## Consequences

- **A command added later has its answer before it is written.** M6.4b's
  screenshot is a read of the world, so it is answered during a replay; anything
  that moves state is refused, and the `consequence` clause is the only new
  prose. `answer`'s exhaustive match has no `_` arm, so a command cannot acquire
  either behaviour by omission.
- **It is a behaviour change, not only a new rule.** A `set_component` over a
  socket attached to a replay used to succeed. Anything relying on that stops
  working, which is the intent.
- **It does not supersede ADR-0012 or its M6.2 amendment.** D19 says a run that
  *accepts* a write has its band cut; this says a replay accepts none, so D19
  never fires there. The two are complementary and the live case is untouched.
- **The reads keep their exactness.** `Inbox::answer_pending` answers in arrival
  order against the state each previous request left, replay or not, and
  `an_entity_answer_is_the_canonical_dumps_block_for_that_entity` is unaffected.
- **The refusal is not the same as being unable to look at a written world.** An
  agent that wants the what-if runs the replay with `--ticks N` to reach the
  state, and then a live run from there — which is not something the engine can
  do today, and is named here rather than left to be discovered. Nothing in this
  ADR forecloses it: a command that turned a finished replay into a live run
  would be a live run being steered, which is what this rule already permits.

## Verification

- `during_a_replay_every_steering_command_is_refused_and_the_reads_are_not`
  (`ipc.rs`) drives all four refusals and both reads, asserts each message's
  prefix, its own clause and the shared tail, and checks that the world, the
  band, the band's openness and the budget are all where they were.
- `a_write_over_the_wire_during_a_replay_is_refused_and_the_replay_is_intact`
  (`tests/agent_socket.rs`) closes the hole where it was — over a real socket, in
  a process, on a command line a user can type — and compares the talked-to
  replay's reported state hash against a plain `--replay` of the same file.
- `a_step_during_a_replay_is_refused_with_its_reason` (`ipc.rs`) pins the
  widened wording and shows M6.3c's clause intact inside it.
- Red edge (c), M6.4a: removing the check from `set` turns exactly those two
  tests red — one in the headless configuration, one in `--features ipc` — and no
  others.
