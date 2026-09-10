// Real authored assets, controlled Memory fixtures. No live Memory/inference operations.
const http = require('node:http'), fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { createRequire } = require('node:module');
const { chromium } = createRequire(process.env.SIGIL_PLAYWRIGHT_PACKAGE)('playwright');
const root = path.resolve(__dirname, '../src/browser/assets'), out = path.resolve('target/d2-screenshots');
fs.mkdirSync(out, { recursive: true });
const actor = { key: 'human:fixture', principal_kind: 'oidc', issuer: 'https://fixture.test', subject: 'fixture' };
const overlay = () => ({ revision: '0', pinned: false, archived: false, annotations: [], updated_by: null, updated_at: null });
const id = 'm_00000000-0000-4000-8000-000000000001';
const chunk = (id, text) => ({ id, idx: 'atlas', text, source: 'git', ref: 'https://example.test/controlled-fixture', path: 'docs/decision.md', span: '1-4', ts: 1789041600, sha: 'fixture-indexed-abc', tags: ['controlled-fixture'], target: { kind: 'document', source: 'git', ref: 'https://example.test/controlled-fixture', path: 'docs/decision.md' }, curation: overlay() });
let manual = { id, revision: '9007199254740993', text: 'A controlled manual memory', tags: ['controlled-fixture'], actor, created_by: actor, updated_at: 1789041600, deleted: false, projection: 'pending' };
const imported = chunk('source1', 'Imported controlled fixture: <img src=x onerror=alert(1)>');
// Synthetic provenance only; unsafe references must stay inert text.
const unsafeSourceLinks = new Map([
    ['unsafe-javascript', 'javascript:window.__unsafeMemoryLink=true'],
    ['unsafe-data', 'data:text/html,<script>window.__unsafeMemoryLink=true</script>'],
    ['unsafe-http-userinfo', 'http://fixture-user:fixture-password@example.test/controlled-source'],
    ['unsafe-https-userinfo', 'https://fixture-user:fixture-password@example.test/controlled-source'],
].map(([id, ref]) => {
    const record = chunk(id, 'Controlled unsafe source-reference fixture ' + id);
    record.ref = ref;
    record.target.ref = ref;
    return [id, record];
}));
const legacy = chunk('legacy1', 'Legacy manual provenance');
legacy.source = 'manual';
legacy.ref = 'memory:' + id;
legacy.target = { kind: 'document', source: 'manual', ref: legacy.ref, path: '' };
let searchCalls = [];
let auth = true, who = 'human:fixture', ready = true, writes = [], created = new Map(), delay = null, definite = false, uncertain = false, detailDelay = 0, previewStale = false, forgetUnknown = false, echo = false;
const session = () => ({ identity: { display_name: 'Controlled Memory fixture' }, actor: { driver: who }, csrf_token: 'csrf', features: { memory_adapter: true } });
const server = http.createServer(async (req, res) => {
    try {
        const u = new URL(req.url, 'http://fixture');
        let raw = '';
        for await (const c of req)
            raw += c;
        const b = raw ? JSON.parse(raw) : null;
        const reply = (v, s = 200) => { res.writeHead(s, { 'content-type': 'application/json' }); res.end(JSON.stringify(v)); };
        if (u.pathname === '/browser/session')
            return reply(auth ? session() : { error: 'login_required' }, auth ? 200 : 401);
        if (u.pathname.startsWith('/browser/api/')) {
            if (!auth)
                return reply({ error: 'login_required' }, 401);
            if (req.method !== 'GET') {
                assert.equal(req.headers['x-sigil-csrf'], 'csrf');
                writes.push({ path: u.pathname, body: b });
            }
            if (u.pathname === '/browser/api/memory')
                return reply({ indexes: [{ name: 'atlas', rows: 3 }, { name: 'notes', rows: null }], readiness: ready ? 'ready' : 'update_or_auth_setup_required', observed_at: 1789041600, project_association: 'unavailable_pending_enrollment' });
            if (u.pathname.endsWith('/ingests'))
                return reply({ sources: [{ source_id: 'known1', source: 'git', ref: 'https://example.test/controlled-fixture', path: 'docs', head: 'fixture-indexed-abc', chunks: 1, last_run: '2026-09-10T12:00:00Z' }], running: null, runs: [{ id: 'ingest1', status: 'failed', error: 'Controlled fixture indexing failure', started: '2026-09-10T12:00:00Z' }], last_ingest: null });
            if (u.pathname.endsWith('/reindex'))
                return reply({ status: 'running' }, 202);
            if (u.pathname.endsWith('/chunks')) {
                if (u.searchParams.get('q')) {
                    searchCalls.push(u.searchParams.get('mode') || 'bm25');
                    return reply({ items: Array.from({ length: 45 }, (_, i) => chunk('search' + i, 'Search fixture ' + i)), next_cursor: null, consistency: 'bounded_search_candidates' });
                }
                if (echo)
                    return reply(JSON.parse(fs.readFileSync('target/d2-adapter-echo.json', 'utf8')));
                return reply({ items: u.searchParams.has('cursor') ? [legacy] : [imported, { ...chunk(id, manual.text), target: { kind: 'manual', id }, source: 'manual', ref: 'memory:' + id, path: '' }], next_cursor: u.searchParams.has('cursor') ? null : 'fixture-page-2', scanned: 2, consistency: 'live_keyset' });
            }
            if (u.pathname.includes('/chunks/')) {
                if (detailDelay)
                    await new Promise(r => setTimeout(r, detailDelay));
                const fixture = unsafeSourceLinks.get(u.pathname.split('/').pop());
                return reply(fixture || (u.pathname.endsWith('legacy1') ? legacy : imported));
            }
            if (u.pathname.endsWith('/history'))
                return reply({ items: [manual], next_after: null });
            if (u.pathname.endsWith('/manual') && req.method === 'POST') {
                if (definite) {
                    definite = false;
                    return reply({ error: 'memory_request_rejected' }, 400);
                }
                const m = { ...manual, id: 'm_' + b.create_id, text: b.text, tags: b.tags, revision: '1', projection: 'pending' };
                created.set(m.id, m);
                if (delay)
                    await delay;
                if (uncertain) {
                    uncertain = false;
                    return reply({ error: 'memory_unavailable' }, 502);
                }
                return reply(m, 202);
            }
            if (u.pathname.includes('/manual/')) {
                const mid = u.pathname.split('/').pop();
                let m = mid === id ? manual : created.get(mid);
                if (!m)
                    return reply({ error: 'memory_record_not_found' }, 404);
                if (req.method === 'PUT') {
                    if (b.expected_revision !== m.revision)
                        return reply({ error: 'memory_revision_or_preview_conflict' }, 409);
                    manual = { ...m, text: b.text, tags: b.tags, revision: (BigInt(m.revision) + 1n).toString(), projection: 'failed' };
                    return reply(manual, 202);
                }
                return reply({ memory: m, target: { kind: 'manual', id: mid }, curation: imported.curation });
            }
            if (u.pathname.endsWith('/curation')) {
                if (b.expected_revision !== imported.curation.revision)
                    return reply({ error: 'memory_revision_or_preview_conflict' }, 409);
                Object.assign(imported.curation, { revision: (BigInt(imported.curation.revision) + 1n).toString() }, b.pinned === undefined ? {} : { pinned: b.pinned }, b.archived === undefined ? {} : { archived: b.archived });
                if (b.annotation)
                    imported.curation.annotations.push({ text: b.annotation, actor, at: 1789041600, context: { chunk_id: 'source1', span: '1-4', sha: 'old-fixture-sha' } });
                return reply({ target: b.target, curation: imported.curation });
            }
            if (u.pathname.endsWith('/forget/preview'))
                return reply({ selector: b.selector, affected: 1, sample: [{ id: 'source1', path: 'docs/decision.md' }], confirmation: 'exact-fixture-confirmation', index_version: '9007199254740993', curation_version: '9007199254740994' });
            if (u.pathname.endsWith('/forget')) {
                if (forgetUnknown) {
                    forgetUnknown = false;
                    return reply({ error: 'memory_unavailable' }, 502);
                }
                if (previewStale) {
                    previewStale = false;
                    return reply({ error: 'memory_revision_or_preview_conflict' }, 409);
                }
                assert.equal(b.confirmation, 'exact-fixture-confirmation');
                return reply({ suppressed: true, selector: b.selector, affected: 1, projection: { pending: 1, failed: 0 } });
            }
            return reply({ error: 'unknown_fixture_route' }, 404);
        }
        const asset = u.pathname.startsWith('/browser/assets/') ? u.pathname.split('/').pop() : 'index.html';
        let data = fs.readFileSync(path.join(root, asset));
        res.writeHead(200, { 'content-type': asset.endsWith('.js') ? 'text/javascript' : asset.endsWith('.css') ? 'text/css' : 'text/html' });
        res.end(data);
    }
    catch (e) {
        res.writeHead(500);
        res.end(String(e));
    }
});
(async () => {
    await new Promise(r => server.listen(0, '127.0.0.1', r));
    const base = `http://127.0.0.1:${server.address().port}`;
    const browser = await chromium.launch({ headless: true });
    try {
        const page = await browser.newPage({ viewport: { width: 1500, height: 1000 } });
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on('pageerror', e => errors.push(String(e)));
        const externalRequests = [];
        await page.route('**/*', route => {
            if (new URL(route.request().url()).origin !== base) {
                externalRequests.push(route.request().url());
                return route.abort();
            }
            return route.continue();
        });
        await page.goto(base + '/ui/memory?index=atlas');
        await page.getByRole('heading', { name: 'Memory', exact: true, level: 1 }).waitFor();
        await page.getByRole('button', { name: 'New manual memory', exact: true }).waitFor();
        await page.getByRole('link', { name: imported.text, exact: true }).click();
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        assert.equal(await page.locator('img').count(), 0);
        assert.equal(await page.getByRole('button', { name: 'Edit source', exact: true }).isDisabled(), true);
        assert.equal(await page.getByLabel('Memory text', { exact: true }).count(), 0);
        const sourceLink = page.getByRole('link', { name: 'Open source reference', exact: true });
        assert.equal(await sourceLink.getAttribute('href'), imported.ref, 'ordinary HTTPS provenance remains navigable');
        assert.equal(await sourceLink.getAttribute('target'), '_blank');
        assert.match(await sourceLink.getAttribute('rel'), /noopener/);
        assert.match(await sourceLink.getAttribute('rel'), /noreferrer/);
        for (const fixture of unsafeSourceLinks.values()) {
            await page.goto(base + '/ui/memory?index=atlas&memory=' + fixture.id);
            await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
            await page.locator('.memory-detail').getByText(fixture.text, { exact: true }).waitFor();
            await page.locator('.memory-provenance').getByText(fixture.ref, { exact: true }).waitFor();
            assert.equal(await page.getByRole('link', { name: 'Open source reference', exact: true }).count(), 0, fixture.id + ' must not become a source link');
            assert.equal(await page.locator('.memory-detail a[href^="javascript:"], .memory-detail a[href^="data:"]').count(), 0);
            assert.equal(await page.evaluate(() => window.__unsafeMemoryLink), undefined);
        }
        assert.deepEqual(externalRequests, [], 'provenance rendering must not fetch any source reference');
        await page.goto(base + '/ui/memory?index=atlas&memory=source1');
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        assert.equal(await page.getByRole('link', { name: 'Open source reference', exact: true }).getAttribute('href'), imported.ref);
        console.log('PASS M1: javascript/data and HTTP/HTTPS userinfo references render as inert provenance text; ordinary HTTPS retains its safe source link; no external requests');
        await page.getByLabel('Annotation', { exact: true }).fill('A correction that survives indexing');
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.waitForTimeout(100);
        assert.equal(await page.getByLabel('Annotation', { exact: true }).inputValue(), 'A correction that survives indexing');
        imported.sha = 'new-controlled-indexed-sha';
        imported.text = 'Updated controlled fixture: <img src=x onerror=alert(1)>';
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.getByText(/The indexed source changed to revision new-controlled-indexed-sha/).waitFor();
        assert.equal(await page.getByLabel('Annotation', { exact: true }).inputValue(), 'A correction that survives indexing');
        await page.getByRole('button', { name: 'Review updated source', exact: true }).click();
        assert.equal(await page.getByLabel('Annotation', { exact: true }).inputValue(), 'A correction that survives indexing');
        await page.getByRole('button', { name: 'Add annotation', exact: true }).click();
        await page.getByText('Curation saved. This does not change search-indexing status.', { exact: true }).waitFor();
        assert.match(await page.locator('.memory-curation').textContent(), /old-fixture-sha/);
        await page.getByRole('button', { name: 'Pin', exact: true }).click();
        await page.getByRole('button', { name: 'Unpin', exact: true }).waitFor();
        await page.getByRole('button', { name: 'Archive', exact: true }).click();
        await page.getByRole('button', { name: 'Restore', exact: true }).waitFor();
        await page.getByRole('button', { name: 'Restore', exact: true }).click();
        await page.getByRole('button', { name: 'Archive', exact: true }).waitFor();
        await page.getByText('Forget from Memory', { exact: true }).click();
        await page.getByRole('button', { name: 'Preview forget', exact: true }).click();
        await page.getByText('Affected records: 1', { exact: true }).waitFor();
        auth = false;
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.getByRole('link', { name: 'Sign in in another tab', exact: true }).waitFor();
        assert.equal(await page.getByRole('button', { name: 'Confirm forget', exact: true }).isDisabled(), true);
        const previewLoginWrites = writes.length;
        auth = true;
        await page.getByRole('button', { name: 'Check sign-in', exact: true }).click();
        await page.waitForTimeout(80);
        assert.equal(writes.length, previewLoginWrites);
        await page.getByLabel('Forget scope').selectOption('source');
        assert.equal(await page.getByRole('button', { name: 'Confirm forget', exact: true }).isDisabled(), true);
        await page.getByRole('button', { name: 'Preview forget', exact: true }).click();
        await page.getByText('Affected records: 1', { exact: true }).waitFor();
        previewStale = true;
        await page.getByRole('button', { name: 'Confirm forget', exact: true }).click();
        await page.getByText(/Request a fresh preview before another explicit confirmation/).waitFor();
        await page.getByRole('button', { name: 'Preview forget', exact: true }).click();
        await page.getByText('Affected records: 1', { exact: true }).waitFor();
        await page.getByRole('button', { name: 'Confirm forget', exact: true }).click();
        await page.getByText(/Forgotten from Memory. Suppression is saved/).waitFor();
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByRole('button', { name: 'Next page', exact: true }).click();
        await page.getByRole('link', { name: 'Legacy manual provenance', exact: true }).click();
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        assert.equal(await page.getByLabel('Memory text', { exact: true }).count(), 0);
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByRole('button', { name: 'First page', exact: true }).click();
        await page.getByRole('link', { name: manual.text, exact: true }).click();
        await page.getByLabel('Memory text', { exact: true }).waitFor();
        await page.getByLabel('Memory text', { exact: true }).fill('Edited controlled manual');
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.getByText('Revision 9007199254740994', { exact: true }).waitFor();
        assert.equal(writes.at(-1).body.expected_revision, '9007199254740993');
        await page.getByText('Saved durably; search indexing failed. Maintenance will retry.', { exact: true }).first().waitFor();
        manual.revision = '9007199254740995';
        await page.getByLabel('Memory text', { exact: true }).fill('Keep my conflicting draft');
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.getByText(/Load the current record/).waitFor();
        await page.getByRole('button', { name: 'Inspect current record', exact: true }).click();
        await page.getByRole('button', { name: 'Use current revision for this draft', exact: true }).waitFor();
        assert.equal(await page.getByLabel('Memory text', { exact: true }).inputValue(), 'Keep my conflicting draft');
        await page.getByRole('button', { name: 'Use current revision for this draft', exact: true }).click();
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.getByText('Revision 9007199254740996', { exact: true }).waitFor();
        await page.getByText('Revision history', { exact: true }).click();
        await page.getByRole('button', { name: 'Load revision history', exact: true }).click();
        await page.locator('.memory-editor article').getByRole('heading', { name: 'Revision 9007199254740996' }).waitFor();
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.screenshot({ path: path.join(out, 'memory-manual-desktop.png'), fullPage: true });
        await page.getByRole('button', { name: 'New manual memory', exact: true }).click();
        await page.getByLabel('Memory text', { exact: true }).fill('   ');
        let count = writes.length;
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        assert.equal(writes.length, count);
        await page.getByLabel('Memory text', { exact: true }).fill('é'.repeat(32769));
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        assert.equal(writes.length, count);
        await page.getByLabel('Memory text', { exact: true }).fill('Rejected draft');
        definite = true;
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.getByText(/Correct it and submit deliberately/).waitFor();
        const rejected = writes.at(-1).body.create_id;
        await page.getByLabel('Memory text', { exact: true }).fill('Corrected draft');
        let release;
        delay = new Promise(r => release = r);
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.waitForTimeout(80);
        const submitted = writes.at(-1).body;
        assert.notEqual(submitted.create_id, rejected);
        await page.getByLabel('Memory text', { exact: true }).fill('Newer unsent text');
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByRole('button', { name: 'New manual memory', exact: true }).click();
        release();
        delay = null;
        await page.getByRole('link', { name: 'Open saved memory', exact: true }).waitFor();
        assert.equal(await page.getByLabel('Memory text', { exact: true }).inputValue(), 'Newer unsent text');
        await page.getByRole('button', { name: 'Start another manual memory', exact: true }).click();
        await page.getByLabel('Memory text', { exact: true }).fill('Ambiguous accepted draft');
        uncertain = true;
        await page.getByRole('button', { name: 'Save memory', exact: true }).click();
        await page.getByText(/The submission identity and exact submitted text are retained/).waitFor();
        const uncertainId = writes.at(-1).body.create_id;
        await page.getByLabel('Memory text', { exact: true }).fill('Retained after uncertainty');
        auth = false;
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.getByRole('link', { name: 'Sign in in another tab', exact: true }).waitFor();
        count = writes.length;
        auth = true;
        await page.getByRole('button', { name: 'Check sign-in', exact: true }).click();
        await page.waitForTimeout(100);
        assert.equal(writes.length, count);
        assert.equal(await page.getByLabel('Memory text', { exact: true }).inputValue(), 'Retained after uncertainty');
        who = 'human:other';
        await page.getByRole('button', { name: 'Retry identical submission', exact: true }).click();
        await page.getByText(/original signed-in actor/).waitFor();
        assert.equal(writes.length, count);
        who = 'human:fixture';
        await page.getByRole('button', { name: 'Retry identical submission', exact: true }).click();
        await page.getByRole('link', { name: 'Open saved memory', exact: true }).waitFor();
        assert.equal(writes.at(-1).body.create_id, uncertainId);
        assert.equal(writes.at(-1).body.text, 'Ambiguous accepted draft');
        assert.equal(created.has('m_' + uncertainId), true);
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByText('Sources and ingestion', { exact: true }).click();
        await page.getByText('Controlled fixture indexing failure', { exact: true }).waitFor();
        await page.getByRole('button', { name: 'Reindex recorded source', exact: true }).click();
        await page.getByText('Reindex requested. Refresh to inspect its actual state.', { exact: true }).waitFor();
        assert.deepEqual(writes.at(-1).body, { source_id: 'known1' });
        await page.getByRole('link', { name: imported.text, exact: true }).click();
        await page.getByLabel('Annotation', { exact: true }).fill('Mobile retained correction');
        await page.setViewportSize({ width: 390, height: 844 });
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.screenshot({ path: path.join(out, 'memory-mobile-detail.png'), fullPage: true });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.evaluate(() => window.scrollTo(0, 0));
        await page.getByRole('link', { name: imported.text, exact: true }).waitFor();
        assert.equal(await page.getByRole('link', { name: imported.text, exact: true }).evaluate(e => e === document.activeElement), true);
        await page.screenshot({ path: path.join(out, 'memory-mobile-browse.png'), fullPage: true });
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
        await page.setViewportSize({ width: 1500, height: 1000 });
        await page.goto(base + '/ui/memory?index=atlas&memory=legacy1');
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        await page.getByText('Forget from Memory', { exact: true }).click();
        await page.getByRole('button', { name: 'Preview forget', exact: true }).click();
        await page.getByText('Affected records: 1', { exact: true }).waitFor();
        forgetUnknown = true;
        await page.getByRole('button', { name: 'Confirm forget', exact: true }).click();
        await page.getByText(/Outcome unknown. Do not repeat this forget/).waitFor();
        count = writes.length;
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByRole('button', { name: 'Next page', exact: true }).click();
        await page.getByRole('link', { name: 'Legacy manual provenance', exact: true }).click();
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        await page.getByText('Forget from Memory', { exact: true }).click();
        assert.equal(await page.getByRole('button', { name: 'Preview forget', exact: true }).isDisabled(), true);
        assert.equal(await page.getByRole('button', { name: 'Confirm forget', exact: true }).isDisabled(), true);
        assert.equal(writes.length, count);
        await page.getByRole('button', { name: 'Back to results', exact: true }).click();
        await page.getByRole('button', { name: 'First page', exact: true }).click();
        detailDelay = 400;
        await page.getByRole('link', { name: imported.text, exact: true }).click();
        await page.getByRole('link', { name: /^notes/ }).click();
        await page.waitForTimeout(500);
        assert.equal(await page.getByRole('heading', { name: 'Source document', exact: true }).count(), 0);
        detailDelay = 0;
        echo = true;
        await page.goto(base + '/ui/memory?index=atlas');
        await page.getByRole('link', { name: 'Echoed [redacted]', exact: true }).waitFor().catch(async (e) => { console.log('Echo page', await page.locator('body').textContent()); throw e; });
        assert.equal((await page.locator('body').textContent()).includes('fixture-memory-bearer'), false);
        echo = false;
        await page.goto(base + '/ui/memory?index=atlas&q=fixture&mode=hybrid');
        await page.getByRole('button', { name: 'Run search', exact: true }).waitFor();
        let searchesBefore = searchCalls.length;
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.waitForTimeout(80);
        assert.equal(searchCalls.length, searchesBefore);
        await page.getByRole('button', { name: 'Run search', exact: true }).click();
        await page.getByRole('link', { name: 'Search fixture 0', exact: true }).waitFor();
        assert.equal(searchCalls.at(-1), 'hybrid');
        await page.getByRole('button', { name: 'Next page', exact: true }).click();
        await page.getByRole('link', { name: 'Search fixture 30', exact: true }).waitFor();
        assert.equal(searchCalls.length, searchesBefore + 1);
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.waitForTimeout(80);
        assert.equal(searchCalls.length, searchesBefore + 1);
        await page.getByLabel('Search mode', { exact: true }).selectOption('vector');
        await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
        await page.getByRole('link', { name: 'Search fixture 0', exact: true }).waitFor();
        assert.equal(searchCalls.at(-1), 'vector');
        await page.goto(base + '/ui/memory?index=atlas');
        await page.getByRole('link', { name: imported.text, exact: true }).waitFor();
        ready = false;
        await page.getByRole('button', { name: 'Refresh', exact: true }).click();
        await page.getByText(/Compatible search remains available; curation is unavailable/).waitFor();
        assert.equal(await page.getByRole('button', { name: 'New manual memory', exact: true }).isDisabled(), true);
        count = writes.length;
        await page.goto(base + '/ui/memory?index=atlas&memory=source1');
        await page.getByRole('heading', { name: 'Source document', exact: true }).waitFor();
        assert.equal(await page.getByRole('button', { name: /^(Pin|Unpin)$/ }).isDisabled(), true);
        assert.equal(writes.length, count);
        assert.deepEqual(errors, []);
        console.log('PASS: actual adapter echo rendered; provenance/manual identity; independent CAS/large revisions/history; pin/archive/restore; scoped preview/stale/commit; create validation and definite correction; uncertain identity/newer edits/navigation/login/actor; ingestion; late selection; old contract; desktop/mobile');
    }
    finally {
        await browser.close();
        server.close();
    }
})().catch(e => { console.error(e); process.exitCode = 1; server.close(); });
