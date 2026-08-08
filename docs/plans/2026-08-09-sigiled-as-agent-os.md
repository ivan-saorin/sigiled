# SIGILED as an OS for LLM agents — design note (2026-08-09)

**Session**: 46a94339 (elevated, driver sigiled-kimi) · **Status**: discussion
capture — **nothing here is ratified**. Candidate directions await il Re.

## 1. Origin

Chat 2026-08-09, operator + sigiled-kimi. The operator's observation: sigilled
came out, ironically, as **an operating system for LLM agents** — a thing he
had tried to build top-down some time ago, and failed. His diagnosis of the
difference: for it to work, it had to **completely disappear from view**.

That is the defining trait of an OS: nobody sees the kernel, everybody sees
programs. And the growth pattern matches history: Unix was grown bottom-up
from a real workload (Space Travel on a scavenged PDP-7); Multics was designed
top-down and collapsed under its own design. Operating systems are grown, not
drawn. sigilled grew from driving real projects — tomes, torchio,
past-claude-chats are the programs; the orchestrator became the floor under
them.

## 2. The mapping — what already exists

| sigilled piece | OS concept |
|---|---|
| sessions / jobs | processes / cron-run daemons |
| the reaper | OOM killer |
| git-backed workspace + merge debt | journaled filesystem + fsck surfaced to the next boot |
| branches | copy-on-write — and swap: session state survives reaping because it was committed = hibernation |
| `elevate` + approvals | sudo + polkit |
| searxng (`search.`) | the NIC / resolver — `/dev/net` |
| folio | the FPU: an LLM doing arithmetic in-context is a CPU emulating floats in software; folio is the math coprocessor — exact, cheap, offloaded |
| skills | installed binaries in PATH |
| the verb set | the syscall table |
| the skill's field-notes section | `dmesg` |
| edge swapping `X-Session-Token` into `Authorization` | the seed of a secrets broker — the agent holds a handle, never the secret |
| `GET /sigiled/contract` | `/proc` + man pages: a self-describing machine — the reason any model boots as a driver with zero training |

## 3. Layers — the kernel test, corrected

The first formulation ("does every session need it merely to exist?") was too
strict; the operator caught it with searxng: no session needs search *to
exist*, yet every session may need to search (for free) without reinventing
the wheel. Three layers instead:

- **Kernel** — what every session needs merely to exist: session lifecycle,
  the git journal, auth/elevate, the contract itself.
- **Runtime, always linked** — search, fetch, folio, journal access. The
  criterion is not "needed to exist" but **"would a session re-derive this
  badly on its own?"** This layer is a **microcode patch for the LLM
  processor's errata sheet**: confabulates → search grounds it; can't
  multiply → folio computes exactly; amnesic between boots → the journal
  remembers. searxng is not a peripheral; it belongs to the errata
  workaround set — the libc every process links without thinking about it.
- **Services** — optional daemons, attached by the same mechanism
  (a registry, a training queue, …).

A capability is done when the model never has to reason about it — one verb,
no theology. That invisibility is why the architecture "came out" as an OS.

## 4. Candidate directions — NOT ratified

Each with the operator's position as registered in chat.

1. **Async IPC = a better journal, not a mailbox.** The operator's correction:
   sessions are async — you cannot ask a session that closed half an hour
   ago. Async IPC is letters, not phone calls (Unix `mail(1)`, `.plan`,
   MOTD). The upgrade is structured journal entry types: intent / files
   touched / decision *with rejected alternatives* / **open questions**. The
   open-questions section IS the mailbox for future sessions; the `open`
   ritual (rule 1) is where the next session reads its mail. No new
   machinery, one new convention.
2. **Timers.** Jobs are already cron-run batch (contract §7). The open gap is
   agent-facing timers carrying an **intent payload**: the waking session
   boots cold and must know *why* it was woken, not just *that*. Operator:
   *to be studied*.
3. **Secrets.** Two different problems: *who is calling* — solved, Authentik
   (per-driver client_credentials, groups claim; any new stack service
   registers as an IdP application); *keys to other people's houses* (GitHub
   PATs, third-party API keys) — the edge-swap pattern (the agent holds a
   handle, the edge injects the secret) generalizes when needed.
4. **Accounting.** Deprioritized by the operator: a personal OS, not a
   corporate one. Minimal form if ever wanted: token counts appended to log
   entries — the journal again, no infrastructure.
5. **Package manager.** Operator: *"this is good."* The package format
   already exists: frontmatter + SKILL.md. Registry = an index on automa;
   install = fetch-and-drop. The interesting field is `depends:` — not on
   other packages, on **stack services** (`needs: [search, folio]`).
   A cartridge declares its senses.
6. **Compute nodes / batch queue.** Operator: *we'll get there.* Cartridge
   training on the i9 is a batch job looking for a scheduler. Note the queue
   can begin as a journal entry type (`intent: run-on-i9-when-free`) — in an
   async world everything collapses into the journal.
7. **Object store via `$ref`.** Operator: *"sort of `$ref: https://…`"*. Git
   holds the pointer, the object lives content-addressed elsewhere. One
   rule: **the hash travels with the ref** — URL is location, hash is
   integrity. Git LFS, but REST-dumb, no smudge filters.
8. **Recipes — the meta-chain.** Operator's example: *ask for something cool
   → search arXiv for genuine novelty → read related papers → pick one
   approach → ADHD-skill decision → build in one shot.* Paper search mounts
   as another runtime resolver, `/dev/papers` next to `/dev/search`
   (OpenAlex as the paper DNS, arXiv for full text). The chain itself is
   **not a module but a recipe**: userland composition of primitives, stored
   as a versioned artifact, executed via `run` — recipes are files, so
   recipes can write recipes. Guardrail: the decision step must write **what
   was rejected and why** into the journal, or novelty-search becomes a
   confabulation amplifier. The app is the side effect; the adjudication is
   the asset.

## 5. What this asks of il Re

Nothing yet — no DEC is proposed for ratification in this note. The framing
alone has documentation value (it names what the stack already is). If a
first candidate is wanted, the cheapest is **4.1 (journal entry
conventions)**: no new machinery, one convention — and everything else rides
on it (async IPC, the batch queue, adjudication memory).
