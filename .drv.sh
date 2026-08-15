set -eu
cd /workspace
ID="-c user.name=mgr-session -c user.email=mgr@016180.xyz"

cat > /tmp/entry.md <<'ENTRYEOF'
## 2026-08-15 — sessione 1fbfe9a8: DEC-28 ratificata, dispiegata e indurita

- **Dove eravamo:** DEC-28 semplificata (`any-authenticated` default) su master, produzione non toccata, ratifica in sospeso.
- **Cosa è successo fuori dal repo, prima di questa sessione:** il Re ha ratificato in chat e il pattern è andato sul bordo vivo — con tre lezioni pagate sul campo, ora documentate in `deploy/Caddyfile.example`: (1) il caddy dispiegato non lega `{args[N]}` negli snippet ("index is out of bounds" su ogni espansione) → il nome del servizio ora deriva dall'HOSTNAME (`{http.request.host.labels.2}`), zero argomenti, call site invariati; (2) un bind mount di FILE singolo si stacca dall'inode quando l'editor salva via temp+rename — il container rilegge per sempre i byte vecchi; scrivere solo via redirect `cat >`, o montare la directory; (3) contare i domini nella riga di log "automatic TLS certificate management" — un config troncato in transito si carica senza lamentele. Verifica end-to-end a valle: JWT driver legge `genie`/`adhd` `openapi.json` e `search` JSON attraverso il gate (`granted_by: authenticated`); i chiamanti statici regrediti verdi; token spazzatura 401.
- **Fatto in questa sessione:**
  1. **Indurimento `/auth/verify`:** `X-Sigiled-Service` è richiesto SOLO sotto `per-service` (dove è input di policy); sotto il default è informativo e la sua assenza non uccide più il braccio JWT — era esattamente il bug che il 15 sera ha trasformato un warning cosmetico del bordo in un braccio morto. Due test di trasporto a livello handler (mancante+default → 200; mancante+per-service → 500, presente → 403). 110 test verdi.
  2. **`deploy/Caddyfile.example` allineato al dispiegato:** snippet `(bearer)` documentato, `(dual)` con gate inline a doppia credenziale e nome-da-hostname, le tre lezioni operative nel commento "Applying this to a live edge".
  3. **Registro §8: riga DEC-28 scritta** — proposta, emendata dall'operatore (per-callee → any-authenticated default), ratificata e dispiegata lo stesso giorno.
- **Scoperto strada facendo (conta per sde):** `paper-api` non ha MAI avuto un token statico — il container porta solo `OIDC_ISSUER=…/o/paper-api/` e `TRUST_EDGE_HEADER`; valida JWT del SUO issuer. Il `PAPER_TOKEN` in `/opt/sigiled/.env` è un access token OIDC mintato una volta e ormai SCADUTO — per questo ogni sonda 401. Mint fresco dal provider `sigilled-paper` (client_credentials) → autentica. E il catalogo mente di nuovo: `spec: GET /openapi.json` risponde 404 "no such endpoint" — paper serve una landing UI a `/` e nessuno spec endpoint. Conseguenze da sistemare (non in questo repo): `[app.secrets] PAPER_TOKEN` inietterebbe un token morto — servono client_id/secret e mint al volo, o la fiducia nell'edge header; e la riga `spec` di paper in `catalog.json` va corretta.
- **Stato:** control plane con l'indurimento su master (redeploy in coda, lo fa il driver via SSH subito dopo il close); bordo vivo già sul pattern DEC-28; registro e example allineati alla realtà.
- **Prossimo:** redeploy sigiledd + verifica; poi sessione su `sde` — S2: client genie (spec ora leggibile) e client paper (endpoint dalla sorgente del progetto `paper`, visto che lo spec endpoint non esiste).
ENTRYEOF

awk 'NR==FNR { e = e $0 "\n"; next }
     !ins && /^## 2026-/ { printf "%s", e; ins = 1 }
     { print }' /tmp/entry.md docs/log-operativo.md > /tmp/new-log.md
test -s /tmp/new-log.md
mv /tmp/new-log.md docs/log-operativo.md
grep -qP '\xc3\x83' docs/log-operativo.md && echo "MOJIBAKE" || echo "encoding clean"

cat > /tmp/msg.txt <<'MSGEOF'
feat(auth): DEC-28 ratified and live — verify hardened, register row, example aligned with the deployed edge

The operator ratified DEC-28 (any-authenticated default) and the pattern went
to the reference instance's edge the same evening, verified end-to-end: a
driver JWT reads genie/adhd openapi.json and search JSON through the gate,
static callers regression green, garbage 401. Three things land here off the
back of that deploy.

/auth/verify hardening. X-Sigiled-Service is now required ONLY under
per-service, where it is the policy input and authorizing against "" would
deny everyone with a group nobody could hold. Under any-authenticated it is
informational, and hard-requiring it was the bug that turned a cosmetic edge
warning into a dead JWT arm mid-deploy: the deployed caddy does not bind
{args[N]} in snippets, the header went out empty, and the handler 500d the
whole arm. Two transport-level tests pin both directions.

deploy/Caddyfile.example rewritten to match what actually runs: the (bearer)
snippet for machine-only vhosts, (dual) with the two-credential gate inline,
and the callee name derived from the hostname ({http.request.host.labels.2})
instead of a snippet argument — site names ARE catalog names here, and
hostname derivation has no binding failure mode. The "applying this to a live
edge" comment now carries the three operational lessons the deploy paid for:
validate in the container's own image before reloading; a single-FILE bind
mount detaches from editor temp+rename saves and pins the container to stale
bytes; count the domains in the "automatic TLS certificate management" log
line, because a config truncated in transit loads without complaint.

sigiled-v2.md gains the DEC-28 register row: proposed, amended by the
operator (per-callee -> any-authenticated default), ratified and deployed
2026-08-15 — with the aud-vs-groups reasoning and the session caveat
compressed into it.

Also recorded in the log for sde's benefit: paper-api never had a static
token — it validates JWTs from its own issuer, PAPER_TOKEN in the stack env
is a long-expired minted access token (hence every 401), and the catalog's
"spec: GET /openapi.json" is stale: paper serves a landing UI at / and no
spec endpoint at all.

110 tests, fmt clean, clippy clean on touched files.
MSGEOF
git $ID add -A
git $ID commit -q -F /tmp/msg.txt
echo "commit: $(git log -1 --format=%h)"
git push -q origin HEAD && echo PUSHED
git status --short && echo "(clean)"
