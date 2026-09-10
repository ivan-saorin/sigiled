/* Memory workspace: text-only rendering, explicit intent, no mutation replay. */
'use strict';
window.SigilMemory = (() => {
    const drafts = new Map(), forgetOps = new Map(), searches = new Map();
    let restoreFocus = null;
    const searchKey = (n, q) => n + JSON.stringify(["q", "source", "ref", "path", "tag", "since", "until"].map(k => q.get(k) || "")) + "|" + (q.get("archived") || "exclude") + "|" + (q.get("mode") || "bm25");
    const bytes = s => new TextEncoder().encode(s).length;
    const rev = s => typeof s === 'string' && /^(0|[1-9][0-9]*)$/.test(s) && BigInt(s) <= 9223372036854775807n;
    const api = n => '/browser/api/memory/indexes/' + encodeURIComponent(n);
    const known = v => v === null || v === undefined ? 'Unknown' : String(v);
    const err = e => ({ memory_revision_or_preview_conflict: 'The record or preview changed. Inspect the current revision before another action.', memory_service_update_or_auth_setup_required: 'Memory needs a service update or authentication setup. Compatible search remains available.', memory_authorization_required: 'Sign in again, inspect the result, then choose a deliberate action.', memory_record_not_found: 'This record is missing, suppressed, or no longer the current indexed projection.', memory_search_filter_unsupported: 'Search supports source, reference, tag and dates. Use Browse for path or archived records.', memory_recorded_source_changed: 'The recorded source changed. Refresh sources before reindexing.', memory_request_rejected: 'The service rejected this submission before saving. Correct it and submit deliberately.', invalid_memory_input: 'The input is not valid. Check the text, tags and revision.' })[e.message] || 'Could not confirm the result. Keep this draft and inspect its current record before retrying.';
    const safe = s => {
        try {
            const u = new URL(s);
            return ['https:', 'http:'].includes(u.protocol) && !u.username && !u.password ? u.href : null;
        }
        catch {
            return null;
        }
    };
    function shell() {
        const S = window.Sigil, { el, request, navigate, checkSession } = S;
        const b = (name, fn, disabled = false) => el('button', { type: 'button', onclick: fn, disabled: disabled ? '' : null }, name);
        const p = text => el('p', {}, text), note = text => el('p', { class: 'memory-notice', role: 'status' }, text);
        const root = el('section', { id: 'memory-root', class: 'memory-workspace' }), status = note('Loading Memory…'), layout = el('div', { class: 'memory-layout' }), indexes = el('nav', { class: 'memory-indexes', 'aria-label': 'Memory indexes' }), results = el('section', { class: 'memory-results', 'aria-label': 'Memory results' }), detail = el('section', { class: 'memory-detail', 'aria-label': 'Selected memory', tabindex: '-1' });
        const q = new URLSearchParams(location.search), n = q.get('index'), selected = q.get('memory'), manualRoute = q.get('kind') === 'manual', isNew = q.get('new') === '1';
        root.append(status, layout);
        layout.append(indexes, results, detail);
        let epoch = 0, ready = false, browseObserved = false, detailData = null, view = null, ingestEpoch = 0;
        const alive = t => root.isConnected && t === epoch;
        function go(changes) {
            const u = new URL(location.href);
            u.pathname = '/ui/memory';
            for (const [k, v] of Object.entries(changes)) {
                if (v === null || v === undefined || v === '')
                    u.searchParams.delete(k);
                else
                    u.searchParams.set(k, v);
            }
            navigate(u.pathname + u.search);
        }
        function focusPanel() {
            if (selected || isNew) {
                layout.classList.add('show-detail');
                if (matchMedia('(max-width: 1000px)').matches)
                    detail.focus();
            }
        }
        const selectionLink = (row) => el('a', { href: '/ui/memory?' + new URLSearchParams({ ...Object.fromEntries(q), memory: row.target?.kind === 'manual' ? row.target.id : row.id, kind: row.target?.kind === 'manual' ? 'manual' : 'document', new: '' }).toString(), 'data-memory-id': row.id }, row.text?.slice(0, 140) || row.id);
        const header = el('div', { class: 'memory-result-header' }, el('h2', {}, n || 'Choose an index'));
        const newButton = b('New manual memory', () => go({ new: '1', memory: null, kind: null }), true);
        header.append(newButton);
        results.append(header);
        const form = el('form', { class: 'memory-filters', onsubmit: e => {
                e.preventDefault();
                const data = Object.fromEntries(new FormData(form));
                if (data.q)
                    searches.set(searchKey(n, new URLSearchParams(data)), { requested: true });
                go({ ...data, cursor: null, offset: null, memory: null, kind: null, new: null });
                if (root.isConnected)
                    root.load();
            } });
        for (const [key, title, type] of [['q', 'Search memories', 'search'], ['source', 'Source', 'text'], ['ref', 'Reference', 'text'], ['path', 'Path', 'text'], ['tag', 'Tag', 'text'], ['since', 'From date', 'date'], ['until', 'Through date', 'date']]) {
            const input = el('input', { name: key, type, value: q.get(key) || '', maxlength: key === 'q' ? 2048 : null });
            form.append(el('label', {}, title, input));
        }
        const archived = el('select', { name: 'archived', 'aria-label': 'Visibility' }, ['exclude', 'include', 'only'].map(v => el('option', { value: v }, v === 'exclude' ? 'Active records' : v === 'include' ? 'All records' : 'Archived records')));
        archived.value = q.get('archived') || 'exclude';
        const mode = el('select', { name: 'mode', 'aria-label': 'Search mode' }, el('option', { value: 'bm25' }, 'Keyword'), el('option', { value: 'vector' }, 'Semantic'), el('option', { value: 'hybrid' }, 'Hybrid'));
        mode.value = q.get('mode') || 'bm25';
        form.append(el('label', {}, 'Search mode', mode));
        form.append(el('label', {}, 'Visibility', archived), el('button', { type: 'submit' }, 'Apply filters'), b('Browse', () => go({ q: null, cursor: null, offset: null })));
        results.append(form);
        if (q.get('q'))
            results.append(b('Run search', () => { searches.set(searchKey(n, q), { requested: true }); root.load(); }));
        const list = el('div', { class: 'memory-list' }), paging = el('div', { class: 'memory-paging' });
        results.append(list, paging);
        const ingestion = el('details', { class: 'memory-ingestion' }, el('summary', {}, 'Sources and ingestion'), p('Open to inspect recorded sources and recent ingestion history.'));
        results.append(ingestion);
        ingestion.addEventListener('toggle', () => {
            if (ingestion.open)
                loadIngestion();
        });
        const back = b('Back to results', () => {
            const old = selected;
            restoreFocus = { index: n, id: old };
            go({ memory: null, kind: null, new: null });
            requestAnimationFrame(() => document.querySelector(`[data-memory-id="${CSS.escape(old || '')}"]`)?.focus());
        });
        detail.append(back);
        if (!selected && !isNew)
            detail.append(el('h2', {}, 'Provenance and corrections'), p('Choose a memory to inspect its source, indexed revision and annotations.'));
        async function freshActor(d) {
            const session = await checkSession();
            if (d.actor && d.actor !== session.actor.driver)
                throw new Error('different_actor');
            if (!d.actor)
                d.actor = session.actor.driver;
            return session;
        }
        function actorFailure(e) { return e.message === 'different_actor' ? 'This draft belongs to the original signed-in actor. Sign in as that actor to inspect or retry.' : err(e); }
        function draft(key, initial) {
            if (!drafts.has(key))
                drafts.set(key, { ...initial, key, busy: false, actor: S.session()?.actor?.driver || null });
            return drafts.get(key);
        }
        function projection(m) { return m.deleted ? 'Forgotten manual memory. Its history remains retained.' : m.projection === 'indexed' ? 'Saved durably; current revision is indexed.' : m.projection === 'pending' ? 'Saved durably; waiting for search indexing.' : m.projection === 'failed' ? 'Saved durably; search indexing failed. Maintenance will retry.' : 'Search indexing status is unknown.'; }
        function renderManual(m, authoritative) {
            const key = n + '|manual|' + (isNew ? 'new' : selected), d = draft(key, { text: m?.text || '', tags: (m?.tags || []).join(', '), base: m?.revision || null, create_id: isNew ? crypto.randomUUID() : null, submission: null, accepted: null, message: '' });
            if (view?.key === key) {
                view.observe(m);
                return;
            }
            const title = el('h2', {}, isNew ? 'New manual memory' : 'Manual memory'), message = note(d.message || (m ? projection(m) : 'Drafts stay in this open tab during navigation and sign-in. Reloading or closing the tab discards unsaved drafts.'));
            const text = el('textarea', { 'aria-label': 'Memory text', rows: 11 }), tags = el('input', { 'aria-label': 'Memory tags' });
            text.value = d.text;
            tags.value = d.tags;
            text.addEventListener('input', () => d.text = text.value);
            tags.addEventListener('input', () => d.tags = tags.value);
            const revisionLine = p('Revision ' + known(m?.revision)), projectionLine = p(m ? projection(m) : 'Not saved'), conflict = el('div', {}), actions = el('div', { class: 'memory-actions' });
            const save = b(d.submission ? 'Retry identical submission' : 'Save memory', () => saveDraft()), inspect = b('Inspect current record', () => inspectDraft());
            const copyMessage = () => {
                if (root.isConnected && view?.key === key) {
                    message.textContent = d.message;
                    save.textContent = d.accepted ? 'Saved' : d.submission ? 'Retry identical submission' : 'Save memory';
                    save.disabled = d.busy || !ready || !!d.accepted;
                    inspect.disabled = d.busy;
                    if (d.accepted && !actions.querySelector('.memory-saved-link'))
                        actions.append(el('a', { class: 'memory-saved-link', href: '/ui/memory?index=' + encodeURIComponent(n) + '&memory=' + encodeURIComponent(d.accepted) + '&kind=manual' }, 'Open saved memory'));
                    if (isNew && d.latest) {
                        projectionLine.textContent = projection(d.latest);
                        revisionLine.textContent = 'Revision ' + d.latest.revision;
                    }
                }
            };
            actions.append(save, inspect);
            if (isNew)
                actions.append(b('Start another manual memory', () => {
                    if (d.busy || d.submission && !d.accepted) {
                        d.message = 'Inspect this uncertain save first. Its identity must not be replaced.';
                        copyMessage();
                        return;
                    }
                    drafts.delete(key);
                    renderRouteAgain();
                }));
            const holder = el('div', { class: 'memory-editor' }, title, p('Index ' + n + '; identity ' + (m?.id || d.accepted || 'm_' + d.create_id)), p('Author ' + known(m?.actor?.subject) + '; issuer ' + known(m?.actor?.issuer) + '; updated ' + (m?.updated_at === undefined ? 'Unknown' : new Date(m.updated_at * 1000).toLocaleString())), revisionLine, projectionLine, el('label', {}, 'Memory text', text), el('label', {}, 'Tags, separated by commas', tags), message, conflict, actions);
            detail.replaceChildren(back, holder);
            focusPanel();
            function observe(latest) {
                copyMessage();
                if (!latest)
                    return;
                projectionLine.textContent = projection(latest);
                revisionLine.textContent = 'Current revision ' + known(latest.revision);
                if (d.base && latest.revision !== d.base) {
                    conflict.replaceChildren(note('A newer revision exists. Your unsaved text is preserved.'), b('Use current revision for this draft', () => { d.base = latest.revision; d.submission = null; conflict.replaceChildren(); d.message = 'Current revision selected. Review the preserved text, then save deliberately.'; copyMessage(); }));
                }
                if (isNew && d.accepted)
                    projectionLine.textContent = projection(latest);
            }
            view = { key, observe, sync: copyMessage };
            copyMessage();
            function validate() {
                const tags = d.tags.split(',').map(s => s.trim()).filter(Boolean);
                if (!d.text.trim() || d.text.includes("\0") || bytes(d.text) > 65536 || tags.length > 32 || tags.some(t => bytes(t) > 128 || t.includes("\0")) || (!isNew && !rev(d.base)))
                    throw new Error('invalid_memory_input');
                return tags;
            }
            async function inspectDraft() {
                if (d.busy)
                    return;
                const identity = d.accepted || (isNew ? 'm_' + d.create_id : selected);
                d.busy = true;
                copyMessage();
                try {
                    await freshActor(d);
                    const latest = await request(api(n) + '/manual/' + encodeURIComponent(identity));
                    if (isNew) {
                        d.accepted = latest.memory.id;
                        d.message = projection(latest.memory) + ' Your newer unsent text is retained.';
                        if (root.isConnected && view?.key === key)
                            message.append();
                    }
                    else
                        d.message = 'Current record inspected. Compare revisions before a deliberate save.';
                    d.latest = latest.memory;
                    if (root.isConnected && view?.key === key) {
                        observe(latest.memory);
                    }
                }
                catch (e) {
                    d.message = actorFailure(e) + (e.status === 404 ? ' Absence is not proof that an in-flight save failed. Keep this identity and retry the identical submission deliberately.' : '');
                }
                finally {
                    d.busy = false;
                    copyMessage();
                    document.getElementById("memory-root")?.syncDraft?.();
                }
            }
            async function saveDraft() {
                if (d.busy || !ready || d.accepted)
                    return;
                try {
                    if (!d.submission) {
                        const tags = validate();
                        d.submission = isNew ? { create_id: d.create_id, text: d.text, tags } : { expected_revision: d.base, text: d.text, tags };
                    }
                }
                catch (e) {
                    d.message = err(e);
                    copyMessage();
                    return;
                }
                const submitted = d.submission;
                d.busy = true;
                d.message = 'Saving; keep this draft open until the result is known.';
                copyMessage();
                try {
                    const identity = await freshActor(d);
                    const result = await request(api(n) + '/manual' + (isNew ? '' : '/' + encodeURIComponent(selected)), { method: isNew ? 'POST' : 'PUT', headers: { 'X-Sigil-CSRF': identity.csrf_token }, body: JSON.stringify(submitted) });
                    d.latest = result;
                    d.message = projection(result) + (d.text !== submitted.text ? ' Newer unsent edits remain in this draft.' : '');
                    if (isNew) {
                        d.accepted = result.id;
                    }
                    else {
                        d.base = result.revision;
                        d.submission = null;
                    }
                    if (root.isConnected && view?.key === key) {
                        projectionLine.textContent = projection(result);
                        revisionLine.textContent = 'Revision ' + result.revision;
                    }
                }
                catch (e) {
                    d.message = actorFailure(e);
                    if (['invalid_memory_input', 'memory_request_rejected'].includes(e.message)) {
                        d.submission = null;
                        if (isNew)
                            d.create_id = crypto.randomUUID();
                    }
                    else if (e.status === 409) {
                        d.message += ' Load the current record; the submitted text is retained.';
                    }
                    else
                        d.message += ' The submission identity and exact submitted text are retained.';
                }
                finally {
                    d.busy = false;
                    copyMessage();
                    document.getElementById("memory-root")?.syncDraft?.();
                }
            }
            if (m && authoritative && !m.deleted) {
                curationControls(authoritative.target, authoritative.curation, holder, null);
                const history = el('details', {}, el('summary', {}, 'Revision history'), p('Current indexing status is shown above; history contains revision snapshots.'));
                holder.append(history);
                let after = '0', busy = false;
                const rows = el('div', {}), more = b('Load revision history', async () => {
                    if (busy)
                        return;
                    busy = true;
                    try {
                        const data = await request(api(n) + '/manual/' + encodeURIComponent(selected) + '/history?after=' + encodeURIComponent(after));
                        if (!history.isConnected)
                            return;
                        for (const item of data.items)
                            rows.append(el('article', {}, el('h3', {}, 'Revision ' + item.revision), p(known(item.actor?.subject)), el('pre', { class: 'memory-text' }, item.text)));
                        after = data.next_after;
                        more.disabled = !after;
                    }
                    catch (e) {
                        rows.append(note(err(e)));
                    }
                    finally {
                        busy = false;
                    }
                });
                history.append(rows, more);
            }
        }
        function renderRouteAgain() { S.rerender(); }
        function curationControls(target, curation, holder, contextId) {
            if (!target || !curation || !rev(curation.revision) || !['document', 'manual'].includes(target.kind)) {
                holder.append(note('Curation metadata is unknown. Refresh after the service update.'));
                return;
            }
            const key = n + '|curation|' + JSON.stringify(target), d = draft(key, { text: '', base: curation.revision, message: '', uncertain: false });
            let current = curation;
            const pane = el('section', { class: 'memory-curation' }, el('h3', {}, 'Annotations and visibility'), p(target.kind === 'manual' ? 'Changes affect this manual memory.' : 'Changes affect this source document across its chunks and future indexing.')), message = note(d.message), conflict = el('div', {}), annotations = el('div', {});
            const annotation = el('textarea', { 'aria-label': 'Annotation', rows: 4 });
            annotation.value = d.text;
            annotation.oninput = () => d.text = annotation.value;
            const controls = el('div', { class: 'memory-actions' }), pin = b(current.pinned ? 'Unpin' : 'Pin', () => mutate({ pinned: !current.pinned })), archive = b(current.archived ? 'Restore' : 'Archive', () => mutate({ archived: !current.archived })), annotate = b('Add annotation', () => {
                if (!d.text.trim() || d.text.includes("\0") || bytes(d.text) > 8192) {
                    d.message = 'Write an annotation of at most 8192 UTF-8 bytes.';
                    sync();
                    return;
                }
                mutate({ annotation: d.text, ...(contextId ? { context_chunk_id: contextId } : {}) });
            });
            controls.append(pin, archive, annotate);
            pane.append(p('Pinning is a label; it does not promise higher search ranking.'), annotations, el('label', {}, 'Annotation', annotation), message, conflict, controls);
            holder.append(pane);
            forgetControls(target, pane);
            function sync() {
                if (!pane.isConnected)
                    return;
                message.textContent = d.message;
                for (const b of [pin, archive, annotate])
                    b.disabled = !ready || d.busy || d.uncertain;
                pane.querySelector('.memory-forget')?.sync?.();
                pin.textContent = current.pinned ? 'Unpin' : 'Pin';
                archive.textContent = current.archived ? 'Restore' : 'Archive';
                annotations.replaceChildren(...current.annotations.map(a => el('article', {}, p(a.text), p('By ' + known(a.actor?.subject) + '; original context ' + known(a.context?.sha) + ' / ' + known(a.context?.span)))));
            }
            pane.observe = latest => {
                if (!latest || !rev(latest.revision))
                    return;
                current = latest;
                if (latest.revision !== d.base || d.uncertain) {
                    conflict.replaceChildren(note('The current annotation revision is ' + latest.revision + '. Your draft remains unchanged. Inspect existing annotations before submitting it again.'), b('Use current annotation revision', () => { d.base = latest.revision; d.uncertain = false; conflict.replaceChildren(); sync(); }));
                }
                sync();
            };
            pane.observe(curation);
            queueMicrotask(sync);
            async function mutate(fields) {
                if (d.busy || d.uncertain || !ready)
                    return;
                const body = { target, expected_revision: d.base, ...fields };
                d.busy = true;
                sync();
                try {
                    const identity = await freshActor(d);
                    const data = await request(api(n) + '/curation', { method: 'POST', headers: { 'X-Sigil-CSRF': identity.csrf_token }, body: JSON.stringify(body) });
                    d.base = data.curation.revision;
                    current = data.curation;
                    d.message = 'Curation saved. This does not change search-indexing status.';
                    if (fields.annotation && d.text === fields.annotation) {
                        d.text = '';
                        annotation.value = '';
                    }
                }
                catch (e) {
                    d.message = actorFailure(e);
                    d.uncertain = ![400, 401, 403, 409, 422].includes(e.status);
                    if (d.uncertain)
                        d.message += ' Inspect current annotations before another submission.';
                }
                finally {
                    d.busy = false;
                    sync();
                }
            }
        }
        function forgetControls(target, pane) {
            const key = n + '|forget|' + JSON.stringify(target);
            if (!forgetOps.has(key))
                forgetOps.set(key, { phase: 'idle', actor: null });
            const op = forgetOps.get(key);
            if (['previewed', 'previewing'].includes(op.phase)) {
                op.serial = (op.serial || 0) + 1;
                op.phase = 'idle';
                op.preview = null;
            }
            const section = el('details', { class: 'memory-forget' }, el('summary', {}, 'Forget from Memory')), scope = el('select', { 'aria-label': 'Forget scope' }, el('option', { value: 'target' }, target.kind === 'manual' ? 'This manual memory' : 'This source document'));
            if (target.kind === 'document' && target.source !== 'manual')
                scope.append(el('option', { value: 'source' }, 'All documents from this source reference'));
            const info = el('div', { role: 'status' }), preview = b('Preview forget', async () => {
                if (op.phase === 'uncertain' || op.phase === 'committing' || !ready)
                    return;
                const selector = scope.value === 'source' ? { kind: 'source', source: target.source, ref: target.ref } : { ...target };
                const serial = ++op.serial;
                op.phase = 'previewing';
                sync();
                try {
                    const identity = await freshActor(op);
                    const result = await request(api(n) + '/forget/preview', { method: 'POST', headers: { 'X-Sigil-CSRF': identity.csrf_token }, body: JSON.stringify({ selector }) });
                    if (!section.isConnected || serial !== op.serial) {
                        if (serial === op.serial)
                            op.phase = 'idle';
                        return;
                    }
                    op.preview = result;
                    op.phase = 'previewed';
                }
                catch (e) {
                    if (serial === op.serial) {
                        op.phase = 'idle';
                        op.message = actorFailure(e);
                    }
                }
                finally {
                    sync();
                }
            }), commit = b('Confirm forget', async () => {
                if (op.phase !== 'previewed' || !ready)
                    return;
                const observed = op.preview;
                op.phase = 'committing';
                sync();
                try {
                    const identity = await freshActor(op);
                    const result = await request(api(n) + '/forget', { method: 'POST', headers: { 'X-Sigil-CSRF': identity.csrf_token }, body: JSON.stringify({ selector: observed.selector, confirmation: observed.confirmation }) });
                    op.phase = 'done';
                    op.message = 'Forgotten from Memory. Suppression is saved; physical cleanup may still be pending. Source files remain.';
                    op.result = result;
                }
                catch (e) {
                    op.phase = e.status === 409 ? 'idle' : e.status === 401 || e.status === 403 ? 'idle' : 'uncertain';
                    op.preview = null;
                    op.message = actorFailure(e) + (op.phase === 'uncertain' ? ' Outcome unknown. Do not repeat this forget; inspect the current record and ingestion state.' : ' Request a fresh preview before another explicit confirmation.');
                }
                finally {
                    sync();
                }
            });
            if (!op.serial)
                op.serial = 0;
            scope.onchange = () => {
                if (['uncertain', 'committing', 'done'].includes(op.phase))
                    return;
                op.serial++;
                op.phase = 'idle';
                op.preview = null;
                sync();
            };
            function sync() {
                if (!section.isConnected)
                    return;
                preview.disabled = !ready || ['uncertain', 'committing', 'previewing', 'done'].includes(op.phase);
                commit.disabled = !ready || op.phase !== 'previewed';
                scope.disabled = ['uncertain', 'committing', 'done'].includes(op.phase);
                info.replaceChildren();
                if (op.preview && op.phase === 'previewed')
                    info.append(p('Affected records: ' + op.preview.affected), el('pre', { class: 'memory-text' }, JSON.stringify(op.preview.sample, null, 2)), p('Preview versions: ' + op.preview.index_version + ' / ' + op.preview.curation_version));
                if (op.message)
                    info.append(p(op.message));
            }
            section.append(p('Preview first, then confirm the exact scope. Imported sources are permanently suppressed from reindexing in this version. Manual text/history is retained as a tombstone. This does not delete source files.'), scope, preview, info, commit);
            pane.append(section);
            section.sync = sync;
            queueMicrotask(sync);
        }
        function renderDocument(data) {
            const key = n + '|document|' + selected;
            if (view?.key === key) {
                const c = detail.querySelector('.memory-curation');
                c?.observe?.(data.curation);
                if (view.sha !== data.sha || view.text !== data.text) {
                    let changed = detail.querySelector('.memory-source-change');
                    if (!changed) {
                        changed = el('div', { class: 'memory-source-change' });
                        detail.prepend(changed);
                    }
                    changed.replaceChildren(note('The indexed source changed to revision ' + known(data.sha) + '. The displayed source and your annotation draft are preserved.'), b('Review updated source', () => { view = null; renderDocument(data); }));
                }
                return;
            }
            const holder = el('div', {}, el('h2', {}, 'Source document'), p('Imported text is read-only. Add an annotation to preserve a correction through reindexing.'), el('pre', { class: 'memory-text' }, data.text));
            const provenance = el('dl', { class: 'memory-provenance' });
            for (const [k, v] of [['Index', n], ['Source', data.source], ['Reference', data.ref], ['Path', data.path], ['Span', data.span], ['Indexed revision', data.sha], ['Indexed time', data.ts === null || data.ts === undefined ? null : new Date(data.ts * 1000).toLocaleString()], ['Tags', data.tags?.join(', ')]])
                provenance.append(el('dt', {}, k), el('dd', {}, known(v)));
            holder.append(el('h3', {}, 'Provenance'), provenance);
            const href = safe(data.ref);
            if (href)
                holder.append(el('a', { href, target: '_blank', rel: 'noopener noreferrer' }, 'Open source reference'));
            holder.append(p('Current checkout: not observed. Project association is not verified; source editing needs project enrollment.'), b('Edit source', () => { }, true));
            curationControls(data.target, data.curation, holder, data.id);
            detail.replaceChildren(back, holder);
            view = { key, sha: data.sha, text: data.text };
            focusPanel();
        }
        async function loadDetail(t) {
            if (isNew) {
                renderManual(null, null);
                return;
            }
            if (!selected)
                return;
            try {
                const data = await request(api(n) + (manualRoute ? '/manual/' : '/chunks/') + encodeURIComponent(selected));
                if (!alive(t))
                    return;
                detailData = data;
                if (manualRoute) {
                    if (data.target?.kind !== 'manual' || data.target.id !== data.memory?.id)
                        throw new Error('invalid_memory_identity');
                    renderManual(data.memory, data);
                    const c = detail.querySelector('.memory-curation');
                    c?.observe?.(data.curation);
                }
                else if (data.target?.kind === 'manual') {
                    go({ memory: data.target.id, kind: 'manual' });
                }
                else
                    renderDocument(data);
            }
            catch (e) {
                if (!alive(t))
                    return;
                status.textContent = (detailData ? 'Showing stale record. ' : '') + err(e);
                if (!view)
                    detail.append(note(err(e)));
            }
        }
        async function loadIngestion() {
            if (!n)
                return;
            const t = ++ingestEpoch;
            try {
                const data = await request(api(n) + '/ingests');
                if (!root.isConnected || t !== ingestEpoch)
                    return;
                const content = el('div', { class: 'memory-source-list' }, p(data.history_limitation || 'Recent process history; completed source records survive restart.'), p('Last ingestion: ' + known(data.last_ingest)), p('Running ingestion: ' + known(data.running?.status)));
                for (const source of data.sources) {
                    const message = note(''), reindex = b('Reindex recorded source', async () => {
                        if (!ready)
                            return;
                        reindex.disabled = true;
                        try {
                            const identity = await checkSession();
                            await request(api(n) + '/reindex', { method: 'POST', headers: { 'X-Sigil-CSRF': identity.csrf_token }, body: JSON.stringify({ source_id: source.source_id }) });
                            message.textContent = 'Reindex requested. Refresh to inspect its actual state.';
                        }
                        catch (e) {
                            message.textContent = err(e) + ' Inspect ingestion before another action.';
                        }
                    });
                    reindex.disabled = !ready;
                    content.append(el('article', {}, p(source.source + ' ' + source.ref), p('Path ' + known(source.path) + '; observed chunks ' + known(source.chunks) + '; indexed revision ' + known(source.head)), reindex, message));
                }
                for (const run of data.runs)
                    content.append(el('article', {}, p('Ingestion ' + run.id + ': ' + run.status), p(run.error || run.detail || 'No additional detail recorded.')));
                if (!data.sources.length)
                    content.append(p('No recorded sources. Reindex is unavailable.'));
                ingestion.querySelector('.memory-source-list')?.remove();
                ingestion.append(content);
            }
            catch (e) {
                if (root.isConnected && t === ingestEpoch)
                    ingestion.append(note('Ingestion observation unavailable. ' + err(e)));
            }
        }
        root.syncDraft = () => view?.sync?.();
        root.load = async () => {
            const t = ++epoch;
            try {
                const data = await request('/browser/api/memory');
                if (!alive(t))
                    return;
                ready = data.readiness === 'ready';
                document.getElementById('connection').textContent = 'Memory observed ' + new Date(data.observed_at * 1000).toLocaleString() + (ready ? '' : '; update or authentication setup required');
                status.textContent = ready ? 'Observed Memory. Browse is a live view; records may change between pages.' : 'Memory needs a service update or authentication setup. Compatible search remains available; curation is unavailable.';
                indexes.replaceChildren(el('h2', {}, 'Indexes'), ...data.indexes.map(i => el('a', { href: '/ui/memory?index=' + encodeURIComponent(i.name), 'aria-current': i.name === n ? 'page' : null }, i.name, el('span', {}, i.rows === null || i.rows === undefined ? 'Count unknown' : i.rows + ' observed chunks'))));
                newButton.disabled = !ready || !n;
                if (!n) {
                    list.replaceChildren(p('Choose an index to browse or search.'));
                    return;
                }
                if (isNew)
                    renderManual(null, null);
                const query = new URLSearchParams();
                for (const key of ['q', 'cursor', 'source', 'ref', 'path', 'tag', 'since', 'until', 'archived', 'mode'])
                    if (q.get(key))
                        query.set(key, q.get(key));
                if (!ready && !q.get('q')) {
                    list.replaceChildren(p('Browse requires the current service contract. Enter search text for compatible legacy reads.'));
                    await loadDetail(t);
                    return;
                }
                try {
                    let page;
                    if (q.get('q')) {
                        const key = searchKey(n, q), semantic = ['hybrid', 'vector'].includes(q.get('mode'));
                        let cached = searches.get(key);
                        if (!cached && semantic) {
                            list.replaceChildren(p('Run this semantic or hybrid search deliberately. Refresh and sign-in do not start model work.'));
                            await loadDetail(t);
                            return;
                        }
                        if (!cached) {
                            cached = { requested: true };
                            searches.set(key, cached);
                        }
                        if (cached.requested) {
                            cached.requested = false;
                            cached.promise = request(api(n) + '/chunks?' + query);
                        }
                        page = await cached.promise;
                        const offset = Math.max(0, Math.min(200, Number(q.get('offset')) || 0)), size = 30, all = page.items;
                        page = { ...page, items: all.slice(offset, offset + size), next_offset: offset + size < all.length ? offset + size : null, next_cursor: null };
                    }
                    else
                        page = await request(api(n) + '/chunks?' + query);
                    if (!alive(t))
                        return;
                    browseObserved = true;
                    const focus = document.activeElement?.getAttribute('data-memory-id');
                    list.replaceChildren(...(page.items.length ? page.items.map(row => el('article', {}, selectionLink(row), p([row.source, row.path].filter(Boolean).join(' / ')), p(row.curation?.archived ? 'Archived' : row.curation?.pinned ? 'Pinned' : ''))) : [p('No records on this page.')]));
                    if (restoreFocus?.index === n) {
                        const previous = list.querySelector(`[data-memory-id="${CSS.escape(restoreFocus.id || '')}"]`);
                        previous?.focus();
                        restoreFocus = null;
                    }
                    if (focus)
                        list.querySelector(`[data-memory-id="${CSS.escape(focus)}"]`)?.focus();
                    paging.replaceChildren(p(page.consistency === 'live_keyset' ? 'Live browse; follow Next even after an empty page.' : 'Search shows at most 200 candidates; this is not an exhaustive total.'), b('First page', () => go({ cursor: null, offset: null }), !q.has('cursor') && !q.has('offset')));
                    if (page.next_cursor !== null && page.next_cursor !== undefined)
                        paging.append(b('Next page', () => go({ cursor: page.next_cursor, offset: null, memory: null, kind: null, new: null })));
                    else if (page.next_offset !== null && page.next_offset !== undefined)
                        paging.append(b('Next page', () => go({ offset: String(page.next_offset), cursor: null, memory: null, kind: null, new: null })));
                }
                catch (e) {
                    if (alive(t)) {
                        status.textContent = (browseObserved ? 'Showing stale results. ' : '') + err(e);
                    }
                }
                await loadDetail(t);
                if (ingestion.open)
                    await loadIngestion();
            }
            catch (e) {
                if (alive(t)) {
                    document.getElementById('connection').textContent = 'Memory observation unavailable; earlier content may be stale';
                    status.textContent = (browseObserved ? 'Showing stale results. ' : '') + err(e);
                }
            }
        };
        queueMicrotask(() => root.load());
        return root;
    }
    return { shell, authExpired: () => {
            for (const op of forgetOps.values())
                if (['previewed', 'previewing'].includes(op.phase)) {
                    op.serial++;
                    op.phase = 'idle';
                    op.preview = null;
                }
            document.querySelectorAll('.memory-forget').forEach(node => node.sync?.());
        } };
})();
