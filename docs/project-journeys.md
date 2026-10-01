# Project collaboration journeys

Source implementation on `feat/project-collaboration-journeys`. Deployment is a separate action. These views extend the existing SIGIL browser and contracts; external agents continue to run in their own hosts.

## Return to a project

`/ui/projects/{project}/overview` starts with a project brief: current system attention, unanswered agent questions, open workspace count, result/Memory links and a bounded recent activity preview. An open workspace is not evidence of an actively running agent. An unavailable question count is shown as unavailable, never zero.

**Mark caught up** records the snapshot time in this browser's local storage, scoped to the authenticated human actor and project. This is a personal reading aid, not a shared workflow state or a claim that the work was accepted. Other browsers do not share the marker. Failure to store it is explicit. Events at the boundary second are included conservatively. Activity is paginated; the brief explicitly says when the preview is incomplete and links to the history. No background request advances the review point.

## Answer an external agent

An authenticated driver creates a question through `POST /sigiled/projects/{project}/requests`:

```json
{
  "id": "f0000000-0000-4000-8000-000000000001",
  "title": "Choose the release scope",
  "question": "Include the new export or defer it?",
  "context": "The reviewed core is ready. Export still needs validation.",
  "source_link": "/ui/projects/example/activity"
}
```

The registered project, authenticated requester, creation time, question and context are durable. Do not put secrets in these records: they follow the existing authenticated shared-project read policy. Titles are at most 200 bytes, question/context 8,000 bytes each and source links 2,048 bytes. IDs use UUID syntax. The machine create route admits up to 128 KiB encoded JSON and rejects unknown fields. Links accept credential-free HTTPS or safe `/ui/` paths; request text is rendered as text, never markup.

Keep the same ID and body after an uncertain create. An exact retry by the same requester recovers the existing record, including a later answer. A different request or requester cannot replace that ID. Questions are immutable in this slice.

Both surfaces read the same records:

| Surface | Routes |
| --- | --- |
| Driver, bearer authenticated | `GET /sigiled/projects/{project}/requests`, `GET /sigiled/projects/{project}/requests/{id}` |
| Human browser, existing B1 identity | `GET /browser/api/projects/{project}/requests`, `GET /browser/api/projects/{project}/requests/{id}` |
| Human answer, exact Origin and CSRF | `POST /browser/api/projects/{project}/requests/{id}/answer` |

Lists take the existing strict `offset`/`limit` pagination, return `waiting`, and sort unanswered questions first. The project **Needs you** tab is `/ui/projects/{project}/requests?request={id}`. It presents requester, context, a source link, answer and receipt. System-derived attention and ordinary tracked work remain accessible from this page.

Answer body:

```json
{"expected_revision": 1, "text": "Defer export; keep this release focused."}
```

Only the authenticated browser can answer. Machine cookies cannot authenticate driver routes, and machine bearer credentials cannot answer through browser routes. There is no machine answer endpoint. Answers are at most 8,000 bytes; the browser route allows 64 KiB encoded JSON. Revision CAS arbitrates competing answers. A repeated exact answer from the same human at the original revision is idempotent. Once answered, changes conflict. The response includes `request.answer.text`, its actor and time; revision becomes 2. The record's `done` state means this question was answered, not that the agent task finished.

The UI preserves drafts across polling and navigation within the open page and scopes them by human/project/request. Reloading the whole page does not preserve unsaved answer text. An uncertain save freezes the submitted body for exact retry or recorded-answer inspection. Sign-in recovery never sends a reply automatically. A changed human identity cannot submit the previous human's draft.

**An answer is not an authorization grant, deployment approval or execution command.** Existing permissions still apply. The external agent reads the answer in its next turn; automatic wake-up is not implied. The UI distinguishes answer recorded from agent continuation observed.

Storage reuses the existing atomic work-item transaction mechanism but writes a separate `agent-requests.json` under `SIGILED_STATE_DIR`. This keeps older work-item writers from discarding request data. It retains one original request and one immutable answer, bounded to 10,000 records per project and 64 MiB per store. Back up this file with the rest of SIGIL state. Storage unavailable/corrupt/uncertain states fail explicitly; a successful write is never inferred from a failed receipt. Ordinary work-item editing cannot alter questions.

## Understand a failed deployment

The existing `/ui/projects/{project}/app` route is labelled **Deployment**. Separate cards show observed source revision, latest completed build revision/result/time, deployed revision and runtime health/observation. A new build in progress is distinct from the last completed build. Stale repository data and revision drift remain explicit.

A failed build does not establish that the old deployment is healthy or down. The overview currently does not probe runtime health or expose build logs, and the UI states that limitation. A diagnostic handoff contains the exact safe recorded evidence and asks an external agent to inspect authoritative build evidence and report a verified fix. Copy failure reveals selectable text. No deploy/retry/rollback action is manufactured from read-only data.

## Validation and release

Run the Rust `project_requests` tests and existing work-item/browser boundary tests, plus `sigiledd/tests/project-journeys.cjs`, `browser-ui.cjs` and `browser-races.cjs`. Browser tests use controlled records and real assets, with configurable `SIGIL_PLAYWRIGHT_PACKAGE`, `SIGIL_BROWSER_EXECUTABLE`, and `SIGIL_SCREENSHOTS`; no real agent, deployment or secret is used. `SIGIL_FIXTURE_ONLY=1` serves the three journeys locally for visual inspection.

Before production adoption, deploy the matching backend and embedded assets through the normal operator-controlled release. Confirm browser Origin/CSRF, driver auth, state persistence and real project navigation on that release. Branch tests and screenshots are not a live deployment claim.
