# Browser research, models and project handoff

Source contract, 2026-09-10. No deployment, live research execution, model lifecycle operation or memory write is established by these fixtures. SDE accepted source is af836b3f9ebef74bfd6624e382fc80ab371251a9; the live binary must independently advertise sde-runs-v1 with durable, nondegraded persistence before new research mutations.

## Browser routes

All APIs below use BrowserContext: verified stable human actor, current server-held original caller access token, exact host/origin, and X-Sigil-CSRF for mutations. Browser cookies/Host are never forwarded. Existing machine APIs remain bearer-only.

| Route | Contract |
|---|---|
| GET /browser/api/research?project=&cursor= | SDE summaries, observed_at, state, readiness, next_cursor. Server requests pages of five. Project filtering uses Sigil's durable actor/run association, not caller-supplied upstream project references. An empty filtered page may have a next page. |
| GET /browser/api/research/{id} | Typed run/stages, full-u64 decimal-string revision, artifact/caller-payload/corpus/dossier/decision/catalog provenance, association and handoff availability. Legacy/global or other-actor records remain unassociated. |
| POST /browser/api/projects/{project}/research | {operation_id,problem,context?,options?}. Explicit Start only. Server reserves a private random UUIDv4 with canonical request before contacting SDE. |
| GET /browser/api/projects/{project}/research/operations | Actor/project-scoped durable acceptance and handoff recovery records. Contains public operation IDs and receipts, never the private SDE key. |
| POST /browser/api/projects/{project}/research/operations/{id}/recover | Explicit same-operation create recovery, with current token and the exact saved request/key. Never automatically resumes. |
| POST /browser/api/research/{id} | {action,expected_revision,output?,model?}. Failed-only resume, or existing categorize/pick/converge stage JSON. Browser revision is a canonical u64 decimal string; submitted stage expected_revision becomes exact numeric JSON for SDE. |
| GET /browser/api/research/{id}/adhd | Follows only the SDE artifact's recorded adhd_run_id. No invented ADHD listing or caller-chosen upstream URL. |
| GET /browser/api/research/{id}/handoff | Exact validated two-file bundle preview, target project, digest; persisted generation-bound preview/recovery metadata takes precedence when an operation already exists. |
| POST /browser/api/research/{id}/handoff | {session_id,generation,bundle_digest}. Generation is a canonical decimal string. Uses existing owned C2 allocation; session/project ownership, original policy, active generation, merge debt and Git handoff are checked before writing. |
| GET /browser/api/models | Independently observed Genie /info, /status and /usage?limit=50; typed slots and optional tokens, per-surface unavailable/authorization states. Read only. |

Global Research is /ui/research; project Research is /ui/projects/{project}/research; Models is /ui/models. Data is text-node/preformatted text, not interpreted model HTML or Markdown links. This intentionally preserves inspectable Markdown source without allowing unsafe links. Lists and detail selections preserve route intent; submitted research drafts are captured before login revalidation and retain later unsent edits. Successful acceptance favors View run; Retry start reuses the submitted snapshot and operation identity. Login recheck never posts automatically. Research attention is derived from current failed/awaiting-caller summaries; it is separate from B2 work items and can clear on the next observation.

## Downstream contract and limits

Reserved sde/adhd/genie bases come from the validated reserved catalog seed; dynamic project declarations cannot override them. HTTPS required in production, no URL credentials/query/fragment, redirects disabled, 2-second connect and 6-second whole-request deadline, response maximum 2 MiB except the exact bare SDE run-detail GET (16 MiB; see Stage E compatibility below). IDs permit bounded ASCII letters/digits/_/-, so they cannot supply paths, query strings, traversal or redirects. Fixed supported operations are enumerated in the adapter. Responses omit raw upstream errors and private key/token fields; current caller tokens are redacted even from a malformed successful service response.

The live health probe requires all accepted sde-runs-v1 capabilities: uuid_v4 operation keys, pagination, durable transitions, expected revision, pinned catalog attempts, and association=unverified, plus persistence.mode=durable, degraded=false, recovery=none. Probe happens on every deliberate research start/recover/resume/stage mutation. Unknown/older or memory-only/degraded services remain readable but mutations return update/storage-repair guidance. Reads cannot open workspaces or invoke inference. New browser research explicitly sends delegate=[] and shape_hint=true; there is no unsupported attached-caller mode. Problem/context are bounded at32768 UTF-8 bytes; options are conservative subsets: papers/category1..10, years0..50, breadth results1..20, aperture1..3. Defaults match SDE for all exposed options. The UI explains that stages run automatically and that starting can use paid providers.

Stage status and degraded evidence are independent. Completed stages and their computed_by/catalog-attempt provenance remain visible during failure or caller waits. Caller deadline and exact prompt payload are inspectable. A completed no-decision run exposes explicit convergence submission; the SDE schema/citation validator remains authoritative. Resume supplies a fresh token and may repeat unfinished work not checkpointed by the service. Browser pre-read revision checks do not extend SDE's machine resume contract into a new external expected-revision parameter.

Genie is a recent observation window, not complete billing or project totals. The underlying ledger may lose records; it has no project attribution. Missing token counts are null/Unknown while a real zero remains zero. SDE/ADHD counts are not added to it. No money, model download/load/unload/bind controls or inferred costs are provided.

## Durable acceptance and recovery

SIGILED_STATE_DIR/research-operations.json stores bounded actor/project/public-operation intent, canonical request, private UUIDv4 and eventual run association together. Single-process mutex transactions reread the file and write mode0600 temporary -> file fsync -> rename -> directory fsync. Newly created directory ancestors are synced. Any uncertain save freezes further mutations in the process, requiring storage repair/restart; reads remain available where valid. Cap10000 operations/32 MiB, no silent eviction of private keys. Restore this file with the platform state, preserving its permissions. It contains no access or refresh bearer.

The same stable verified actor and project can recover after a different temporary browser login. A different actor cannot retrieve or reuse another actor's saved operation. An ambiguous POST never creates a replacement key; deliberate retry uses the same canonical request. The accepted SDE replay precedes catalog refresh and starts no second executor. Read associations do not turn SDE's unverified metadata into verified identity. Project deletion/renaming does not reassign history.

## Handoff and lifecycle authority

The browser previews exactly two Markdown files under docs/design/{validated-slug}/. The deliberate action tells the user to save unsaved buffers and that the editor will pause. C2 allocation is a separate owned-session operation with a stable research-handoff key. Sigil takes the A1 session lock then the project mirror lock; checks actor/project/generation/runtime ownership and merge debt; reads that workspace's Git log; and persists a generation-bound handoff marker before the helper can write.

The companion adds only POST /handoff and GET /handoff-status/{id}, authenticated by the existing generation-specific control credential, with contract sigil-handoff-v1. It is not an exec proxy. It refuses invalid bundles/generations, active user commands and unknown editor ownership. Its detached owning task retains editor/process and synchronization authority until completion, including after the HTTP caller disappears. Terminal status publication belongs to that owning task.

The helper uses a platform lock, clean staged/unstaged/untracked precondition and target nonexistence check before writes. It walks docs/design/slug through no-follow directory descriptors, writes only approved files with exclusive creation, and preserves partial files on failure. Recovery requires the same operation/payload and exact existing partial content; changed partial files are never overwritten. Symlinks and nonregular targets are rejected.

The commit is built from immutable approved content with a PRIVATE Git index rooted at the recorded base. It never invokes the workspace /git/commit endpoint or add -A, and never includes unrelated staged/unstaged content. Before publishing, it checks expected branch, base, index and target content; owns Git's index.lock; updates the session ref using compare-and-swap; publishes the exact commit index; and pushes only the immutable SHA to the trusted session branch URL. Concurrent editor saves remain on disk and produce committed_with_concurrent_changes rather than a false clean result. This does not claim arbitrary filesystem writers obey platform locks.

The helper journal .git/sigil-handoff-{operation}.json records the canonical request/base, immutable commit before ref publication and pushed receipt. Rejected push retains local files/commit and can be retried with the same operation. Incomplete changed writes require user recovery rather than rollback. The browser adapter stores the pushed session commit receipt in the durable research operation and separately marks editor paused. Opening/restarting the IDE is a later explicit action. A session commit is not accepted master or indexed memory; D3 must confirm those transitions independently, using run+commit.

SessionRecord.handoff defaults to None for old snapshots. A pending or unknown-phase marker durably quarantines all IDE mutations, close, recycle and reap; read-only status and same-operation recovery remain available. Timeout, absence, unknown helper state, a replacement generation, or a controller restart cannot clear it. Only an authenticated matching generation/operation terminal helper observation allows completion/failure publication. If the companion itself restarts and has no terminal observation, recovery remains quarantined for explicit operator investigation; there is no unsafe automatic replay or broad clear/exec escape. Older binaries ignore this new field: DO NOT downgrade a controller/helper while any handoff is pending. Back up state.json, research-operations.json and workspace Git journals together.

## Verification qualifications

Fake HTTP engines and actual browser assets only. Focused research/security/recovery/lifecycle tests pass9/9; unknown-editor helper test passes1/1; real-assets browser fixture and existing11-case browser race regression pass. All-target Clippy with only the existing too_many_arguments exception, fmt and diff checks pass.

The initial helper bundle suite passed4/5; its exact replay fixture failed under subprocess termination-unconfirmed. An unchanged isolated check failed before journal creation, and one instrumented diagnostic failed after journal creation but before dossier writes; neither failure reached the later content/index equality checks. Keep exact logs, durations and actual exits in the B3b report. This is unresolved validation, not a fully green handoff suite or proven environmental diagnosis. Earlier Sigil full-suite199/2 and slow lifecycle reliability are also unresolved. No shared supervisor deadline or cleanup policy was changed. A release requires independently reviewing these source changes, resolving the remaining helper/supervisor validation, a durable SDE deployment/live probe, reviewed controller+helper images, actual OIDC/edge/provider/policy readiness, and an authorized real user path check.


### Native Git custody clarification (B3b pre-review follow-up)

The helper imports shared/bounded_process.rs directly. Its Error enum has no
TerminationUnconfirmed variant. The diagnostic "repository subprocess termination
unconfirmed; retaining mirror ownership" is emitted inside Group::finish, which
continues waiting and does not return until waitid, complete process/thread group
inspection and its observer report two quiet passes, then the leader is reaped.
Every error after spawn unwinds through Group::drop and the same cleanup loop.
The control-plane registry's separate outer deadline/error must not be confused
with a return from this helper call.

The custody chain is handoff's detached tokio::spawn owner -> awaited
spawn_blocking handle -> apply -> output_until -> Group. While cleanup is
unconfirmed, apply retains its platform flock (and index lock if reached), the
blocking closure retains the owned sync guard, the detached owner retains the
editor process mutex, and helper observation remains busy. Native Git errors
cannot publish failed before that awaited closure returns. Caller cancellation
drops only a JoinHandle; it does not cancel the detached owning task. A blocking
panic must unwind its Group before joining; an outer panic leaves busy rather
than fabricating a terminal observation. A process abort/restart loses the
in-memory observation and returns unknown for the old operation; it cannot clear
Sigil's durable pending marker.

Sigil persists pending before POST. HTTP timeout/cancellation, busy, unknown and
unrecognized phases preserve it. close, recycle, reaper, IDE mutation and
flush_record reject pending/unknown phases. Matching authenticated recovery alone
may consume the owned helper's terminal status; an arbitrary error response or
missing operation does not release authority. Same-process start/stop/finish are
blocked on the editor process mutex, checkpoint/autosave on sync, and a second
handoff is rejected while busy. Existing code cannot fence arbitrary external
terminal/filesystem writers or a direct privileged workspace API caller; the
handoff protects approved commit content with immutable objects and base/index/
ref-CAS checks. Do not describe the A1 lock as universal writer serialization.
After companion restart, normal Sigil paths remain quarantined; bypassing Sigil
with a directly custodied helper control token is outside that lifecycle gate.

Final filesystem tests exercise the production IndexLock::publish method with
an actual successor index.lock created before the old guard drops, plus failed
publication cleanup; and actual FIFO/directory targets through both recovery
write_bundle and verify_files. The empty FIFO case rejects before content reads,
without waiting for a writer. These tests avoid native Git and passed 2/2. They
do not replace the retained failing full commit replay evidence or establish
its native failure cause. No shared subprocess supervisor or budgets changed.


## B3b fix round 1: accepted boundaries and measured native correction

Research list now scrubs the final serialized projection, including summary and
pagination strings, using the actual forwarded caller bearer. The browser form
validates nonblank problem, UTF-8 byte limits and numeric bounds before freezing
the submitted snapshot. A definite local invalid_research_options rejection
clears that snapshot and chooses a fresh operation identity for a deliberate
corrected submission. Ambiguous service/auth outcomes retain the original
operation and payload; no automatic retry occurs.

The controller and real companion extractor share MAX_REQUEST_BYTES=1_100_000
for serialized UTF-8 JSON. The controller projects exact path/content file DTOs,
serializes the entire helper request and checks the limit before any helper POST
or durable pending marker. An escaped oversized request returns HTTP413
handoff_bundle_too_large_reduce_content with no newly created marker. Prior
genuinely uncertain markers remain quarantined. An integration fixture uses the
actual companion router/extractor/process implementation plus a temporary Git
repository and the observed vm-base /git/log top-level-array contract. It covers
oversize-before-write, rejected log precondition, immutable pushed content,
lost response with pending preserved, matching terminal recovery without repost,
and a normal same-operation receipt. Test-only loopback endpoint injection uses
ephemeral ports; no runtime command runner or authority check is substituted.

The old native path was measured under ~5,700 visible processes. Its unchanged
100 ms full-/proc scan repeatedly exhausted time with most of the 65,536 entry
budget remaining. A fixture rev-parse spent 152,076 ms waiting for confirmed
cleanup, then the next call correctly saw the expired 30 s operation deadline.
The diagnostic ended exit101 at157.17 s without external termination; fixture
files and logs are preserved. Earlier failures remain evidence, not rewritten.

The minimal shared correction keeps full enumeration and both original budgets,
using getpgid to skip unrelated candidates before opening their stat files. Only
ESRCH is treated as a vanished entry; other syscall errors retain uncertainty.
For owned candidates, stat PID/group validation and every-thread inspection are
unchanged. The reserved leader must still be visible; two complete quiet passes,
the observer and leader reap remain required before ownership releases.

The syscall is a membership observation, not PID ownership/adoption. If PID/group
changes between getpgid and stat, the existing stat identity/group checks apply;
the next complete pass re-observes inventory. The unreaped leader reserves its
PID/PGID throughout cleanup. A skipped unrelated/vanished entry does not waive
leader visibility or prove global quiescence. This retains the previous scan's
non-atomic process-change model; it does not promise to fence arbitrary outside
writers. Linux semantics: https://man7.org/linux/man-pages/man2/getpgrp.2.html.

With this correction the strengthened native apply/pushed-SHA/blob/replay/conflict
test passed, followed by handoff7/7 and durable-checkpoint5/5. Process11/11,
ecosystem cancellation1/1, deadline2/2, quarantine1/1, ecosystem observations3/3,
jobs registry deadline1/1, research joins3/3 plus final helper-contract1/1 and the
actual-assets browser checks passed. Positive SDE fixtures establish numeric
9007199254740993 forwarding and fresh-token resume with completed artifacts.
All-target workspace Clippy and format/diff checks pass. These are scoped fix
checks, not a full workspace or live deployment claim; prior199/2 remains historical.


## Stage E response-size compatibility

SDE list requests use five summaries per native cursor page. A supported 32,768-byte problem can expand sixfold in JSON, so the previous fifty-summary request could exceed the fixed 2 MiB transport guard. Native cursors and complete summaries are preserved; no extra executor is called for pagination. The exact bare GET `runs/{id}` response alone permits 16 MiB, matching the durable SDE pretty-serialized record bound (which also includes private metadata). Declared and streamed limits remain enforced. Health, handoff, mutations, ADHD and Genie keep the existing 2 MiB bound; this does not promise that every large machine-created dossier fits the separate native handoff contract. Existing running-binary readiness, original JWT forwarding and redaction are unchanged.

The controlled regression uses actual SDE POST/executor/fake-engine/stage/dossier/GET output. The list diagnostic's twelve summaries exceed the old cap; the actual one-row response includes a native cursor. Subsequent small pages in the consumer cursor test are fixture partitions of those actual summaries, not a claimed native SDE cursor walk. Accepted SDE pagination tests remain the producer's completeness evidence.
