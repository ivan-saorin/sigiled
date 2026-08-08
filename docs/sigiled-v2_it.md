# SIGILED — Documento di design

*(nato come «SIGILED v2», poi «SIGILED» — rinominato 2026-08-03 con DEC-12 emendata; il contratto di guida resta SIGILED)*

**Versione:** 0.3 · **Data:** 2026-08-03 · **Stato:** TUTTE le decisioni ratificate — DEC-01…10 ratificate dal Re il 2026-08-03 (in chat, a piattaforma completa e verificata dal vivo); DEC-11…20, DEC-22 e DEC-23 ratificate in precedenza, DEC-21 registrata; DEC-12 **emendata** 2026-08-03 → piattaforma **SIGILED** (`ivan-saorin/sigiled`, domini `sigiled.dev`/`sigilled.dev`). Il contratto è 2.0.0, non più draft
**Origine:** sessione di design del 2026-08-02 (driver: Kimi K3), partita dalla lettura degli aggiornamenti di `tomes-and-tales` (auth deployata con Authentik) e `torchio` (requisiti v0.3, DEC-17/18): i pattern nati nei progetti **salgano di un livello**, dentro la piattaforma.
**Memoria operativa:** `docs/log-operativo.md` — ogni sessione la aggiorna (convenzione §3).

---

## 0. Sintesi

SIGILED v2 sono **quattro cambiamenti** sorretti da un solo principio: **git è il modello mentale di tutto lo stack**, non solo dei repo.

1. **Auth a due gambe** — i driver LLM diventano client OAuth2 di Authentik (`client_credentials`, identità macchina per-driver); l'approvazione umana arriva via *device flow* con token custoditi **lato SIGILED**, mai nelle skill. Il bearer unico muore.
2. **Log operativo a due layer** — SIGILED espone la storia meccanica dal proprio DB (`GET /sigiled/projects/{p}/log`); il file narrativo `docs/log-operativo.md` nasce dal template e resta per sempre del progetto.
3. **Template versioning** — il recepimento (torchio DEC-17) sale di livello: vm-tmpl versionato con tag, pin in `sigiled.toml`, sync on-demand con drift detection, **mai auto-update**. Il motore serve il proprio contratto (`GET /sigiled/contract`).
4. **Concorrenza a riconciliazione** — addio lock di sessione: un branch per workload, merge al close, e un **merge debt** con pacchetto di contesto che la sessione successiva **deve** risolvere prima di qualsiasi altro lavoro, quale che sia il modello che la guida.

I quattro si compongono: l'auth dà autori veri al log; il log registra update di template e merge debt; il recepimento distribuisce il contratto nuovo ai progetti; la concorrenza rende tutto multi-driver senza serrature.

---

## 1. Auth — due gambe

### 1.1 La dottrina, generalizzata da tnt

Come in tnt (docs/auth.md §1): **l'IdP sa solo membership; ogni applicazione mappa gruppi → capability localmente**. Authentik è l'IdP dello stack, già live su `auth.016180.xyz`. SIGILED diventa una relying party come le altre — niente IdP privato, niente console nuova: Authentik È la console.

### 1.2 Evidenza raccolta dal vivo (2026-08-02)

Verificato con sonde anonime:

- `GET /application/o/tomes-and-tales/.well-known/openid-configuration` → **200**, discovery completa e pubblica (OIDC nasce per essere scopribile).
- Grant dichiarati: `authorization_code`, `refresh_token`, `implicit`, **`client_credentials`**, `password`, **`urn:ietf:params:oauth:grant-type:device_code`** (con `device_authorization_endpoint` dedicato).
- `POST /application/o/token/` senza credenziali → `400 invalid_client`: il token endpoint è vivo e sorvegliato; la porta per le macchine esiste già.
- `POST /application/o/device/` con il solo `client_id` pubblico di tnt → `400 invalid_client`: il device endpoint è attivo; serve un provider configurato per device flow.
- JWKS del provider tnt = `{}` perché firma **HS256** (simmetrico). Per i provider SIGILED: **RS256**, così SIGILED valida i JWT localmente via JWKS.

### 1.3 Gamba machine — client_credentials per driver

- **Un provider OAuth2 per driver** (`sigiled-kimi`, `sigiled-claude`, …): ciascuno con la propria coppia `client_id`/`client_secret`, grant ristretto a `client_credentials`, firma RS256. In Authentik un provider = una coppia → per-driver significa revoca per-driver e audit per-driver.
- **Il driver si auto-minta i token**: `POST /application/o/token/` → access token breve → `Authorization: Bearer` verso `api.016180.xyz`. Scaduto → se ne prende un altro. **Zero umani nel loop dopo il setup.**
- **Concorrenza sicura per costruzione**: ogni token è indipendente, nessuno stato mutabile condiviso fra chat parallele — il problema di rotazione dei refresh token (§1.5) qui non esiste.
- **Nella skill SIGILED**: `client_id` + `client_secret` al posto del bearer monolitico. Stesse regole di handling (mai echo, mai commit), stessa storia di rotazione (401 → chiedi il nuovo valore). Un leak compra solo token brevi di *quel* driver, revocabile in un click.

### 1.4 Gamba umana — device flow, custodia lato SIGILED

Per le operazioni che devono portare «Ivan ha approvato»:

```
1. driver → POST /sigiled/auth/elevate
2. SIGILED (client OIDC) → POST auth.016180.xyz/application/o/device/   [provider sigiled-device]
3. SIGILED → driver → chat: «vai su auth.016180.xyz/device, codice ABCD-1234»
4. operatore approva dal browser (una volta)
5. SIGILED fa polling al token endpoint, ottiene access+refresh token
   → li custodisce nel proprio DB, auto-refresh serializzato nel suo processo
```

- **Access token 12h, refresh 30 giorni** (configurazione provider): l'umano approva ~**una volta al mese per driver**, non 1-2 volte al giorno.
- **Il token non tocca mai né skill né PC né transcript**: SIGILED è l'unico componente con stato persistente e processo sempre vivo — il custode naturale. Niente corse di rotazione: la serializzazione è interna a SIGILED.
- Stampare `user_code` in chat è sicuro per costruzione: autorizza soltanto; i token arrivano a chi detiene il `device_code`, che resta server-side.

### 1.5 Perché non «token di lunga vita nella skill»

Ipotesi valutata e scartata (device flow + auto-refresh con refresh token salvato nella skill):

1. **Corse di rotazione** — Authentik ruota il refresh token a ogni uso; più chat parallele con lo stesso token si invalidano a vicenda, e il problema rientra dalla finestra proprio durante le sessioni parallele.
2. **Leak surface** — la skill viene letta per intero nel contesto a ogni attivazione: un segreto da 30 giorni in ogni transcript di ogni provider.
3. **File ≠ database** — la skill è un documento condiviso fra superfici; farla riscrivere a ogni refresh è fragile o impossibile (alcune superfici non possono scriverla).

### 1.6 Actor e capability

Ogni record session/job guadagna:

```
actor: { driver: "sigiled-kimi", approval: "ivan (device, scade 2026-08-03T01:00)" | null }
```

Capability map v1 (SIGILED-locale):

| | `stack:admins` | `stack:drivers` |
|---|---|---|
| sessions open/close/recycle, git, exec | ✓ | ✓ |
| jobs run/recap | ✓ | ✓ |
| projects new | ✓ | con approval |
| apps verbs (start/stop/restart/upgrade) | ✓ | con approval |
| skill render (`GET /skill/{driver}`, DEC-24) | ✓ | con approval |

### 1.7 Migrazione dal bearer unico

Finestra **dual-auth**: l'edge accetta legacy bearer (= bootstrap admin) e token Authentik; si creano i provider `sigiled-*`, si aggiornano le skill, poi il legacy muore. La skill gestisce già la rotazione via 401: storia compatibile.

### 1.8 Note di sicurezza

- Grant `password` **disabilitato** sui provider SIGILED — solo `client_credentials` (driver) e `device_code` (sigiled-device).
- Secret driver lunghi e generati; verificare rate-limiting all'edge sulla route del token endpoint.
- Validazione in SIGILED: **JWKS + RS256** (locale, niente chiamata per richiesta); introspezione come fallback/debug.
- Claim di gruppo nei token via property mapping Authentik — da verificare in configurazione (serve per `stack:drivers` come claim).

---

## 2. Log operativo — due layer

- **Layer macchina (SIGILED-owned)**: SIGILED ha già tutti i dati (sessioni, close con esito merge, job run, build app) nel proprio DB. Li espone: **`GET /sigiled/projects/{p}/log`**. Zero scritture nei repo dei progetti, zero violazioni della proprietà dei contenuti.
- **Layer narrativo (driver-owned)**: `docs/log-operativo.md` con contratto in testa (tre domande: dove eravamo / dove prevedevamo di andare / cosa è stato fatto + scarti, stato, prossimo passo; voci non si cancellano, si correggono con voci nuove). **Lo skeleton nasce dal template alla creazione del progetto e da quel momento è del progetto, per sempre** — la regola torchio DEC-18 generalizzata. Il template non lo tocca mai dopo la creazione: collisione risolta per costruzione.
- **Regola SIGILED nuova**: *chiudi lavoro coerente → aggiungi una voce in cima al log operativo*.
- **Hint onesto**: `close` risponde con `log_operativo_touched: false` quando il file non è stato modificato — specchio, non enforcement.

## 3. Template versioning — il recepimento sale di livello

La meccanica di torchio DEC-17, applicata a vm-tmpl:

- **vm-tmpl versionato**: tag semver + CHANGELOG.
- **Pin alla creazione**: `sigiled.toml` on master guadagna `template = "vm-tmpl@x.y.z"` — casa naturale, è già il file che SIGILED legge.
- **Recepimento**: allowlist di path SIGILED-owned + `sync` script + **drift detection** (se hai toccato file SIGILED-owned, si ferma e segnala). Rollback = `git revert` o re-pin al tag precedente.
- **Mai auto-update** — simmetria con torchio DEC-07: SIGILED non riscrive mai i repo dei progetti di propria iniziativa. Sync v1 in sessione (close = imbragatura), v2 come job.
- **Visibilità**: `status` mostra `template_behind: true` accanto a `needs_merge`.
- **Il contratto servito dal motore**: **`GET /sigiled/contract`** — il testo SIGILED canonico, versionato. Con `healthz.version` già esistente, ogni driver può verificare la freschezza della propria skill e rigenerarla (l'Appendix A lo prescrive; ora diventa meccanico).

### 3.1 Il workspace v2 — immagine base + ext per linguaggio (ratificato 2026-08-02)

Fatto emerso leggendo il vm-tmpl v1: ogni progetto vendorizza `server/` + `ext/` + `build-ext.sh` + lo stage cargo del `Dockerfile`, e ricompila vm-base a ogni build (con `COPY . .` che sbatte la cache a ogni commit). La v2 lo ribalta:

- **Immagine base pre-buildata per tag**: vm-base è pubblicata come immagine taggata (`vm-base:x.y.z`, registry dello stack). Il Dockerfile del progetto si riduce a `FROM vm-base:x.y.z` + i layer di toolchain del progetto (python, go, …). Il recepimento dell'agent diventa **bump del tag** (punto del pin fissato da DEC-25: `[workspace] dockerfile` in `sigiled.toml` nomina il file, il `FROM` del Dockerfile porta il tag). Dai repo dei progetti spariscono `server/`, `ext/` vendored, `build-ext.sh` e lo stage cargo: con loro muore la ricompilazione a ogni build.
- **Ext per linguaggio**: il punto di estensione si generalizza in `ext-<lang>/`:
  - `ext-rust/` — la convenzione attuale: crate compilate **dentro** vm-base (statico, zero runtime aggiuntivo);
  - `ext-py/`, `ext-go/`, … — processi locali supervisionati nel container; vm-base fa reverse-proxy su porta/socket.
  - Contratto unico invariato: HTTP montato a **`/x/<nome>`**, dentro lo stesso token gate. La toolchain segue l'ext: un progetto con `ext-py/` porta python nell'immagine via layer progetto.
- **vm-tmpl v2** di conseguenza: Dockerfile sottile (FROM + hook toolchain), `docs/` skeleton (incl. log-operativo), `sigiled.toml` commentato, `ext-rust/` di esempio vuota. L'allowlist SIGILED-owned si riduce quasi a zero — scheletro docs e poco più: il grosso del recepimento viaggia sul tag dell'immagine.

## 4. Concorrenza — da esclusione a riconciliazione

### 4.1 Il cambio strutturale

Il lock non sparisce: **si restringe da tutta la vita della sessione a una sezione critica di pochi secondi al merge**. Si paga il costo della coordinazione solo quando i conflitti esistono davvero, e git è costruito per minimizzarli.

1. `open` → branch `session/{id}` da master corrente. **Niente più 409**: N sessioni concorrenti, N container isolati, N token per-sessione (il modello a token già lo supporta, non cambia nulla).
2. Lavoro, commit, push automatico — identico a oggi.
3. `close` → SIGILED acquisisce il merge-lock del progetto (secondi) e tenta in sequenza: **fast-forward** (master fermo: caso comune) → **merge a tre vie** (master mosso ma cambi disgiunti: git fonde da solo) → **conflitto**.
4. Close simultanee: la sezione critica le serializza — una vince, l'altra vede master mosso e va nel ramo merge.

### 4.2 Merge debt

Al conflitto: master resta dov'è, il branch resta, e SIGILED registra il pacchetto di contesto — perché chi risolverà **non ha scritto nessuna delle due metà**:

```json
merge_debt: {
  "branch": "session/…",
  "conflicted_files": ["docs/requisiti.md", "spina/linter.py"],
  "ours":   { "sha": "…", "commit_messages": ["…"] },
  "theirs": { "sha": "…", "commit_messages": ["…"] },
  "since": "…Z"
}
```

I commit message intent-carrying (regola 2) si pagano qui la seconda volta: sono il contesto per decidere.

- **`open` su progetto con debt** → `merge_debt` in cima alla risposta, urlato.
- **Regola dura**: *risolvi il merge debt PRIMA di qualsiasi altro lavoro, quale che sia il modello che ti guida.*
- **Protocollo di risoluzione**: il container parte dal branch in debt col merge in corso e i marker nei file → leggi i commit message dei due lati → risolvi → verifica → committa spiegando *cosa hai tenuto e perché* → chiudi. **Se non sai decidere: chiedi all'operatore, non indovinare.**
- **Merge commit, non rebase**: registrano il confine di sessione e chi ha fuso. La linearità è estetica; quella traccia è memoria.
- Il layer macchina del log registra fallimento e risoluzione; `status` mostra la coda di debt per progetto.

### 4.3 Conflitti semantici — la compilazione di scrupolo

Git può fondere pulito e produrre un risultato rotto (due sessioni toccano parti diverse di codice interdipendente). Regola:

> Una sessione che nota **merge multipli nella storia recente** del progetto esegue la **compilazione di scrupolo** (build/test se il repo li ha; riesame di coerenza dei documenti se è un repo di soli doc). **Se è rotta: DEVE sistemare prima di procedere.**

Futuro (gancio, non v1): hook post-merge opzionale dichiarato in `sigiled.toml` per automatizzare il check.

### 4.4 Le regole riscritte

- **Regola 4** — da «un workload per progetto» a: **un branch per workload; master è l'arbitro al close.** Il 409 sulle sessioni sparisce; resta il divieto di retry-hammer su lock di merge.
- **Regola 7** — da «master si muove solo via close (fast-forward)» a: **master si muove solo via close (FF preferito, merge altrimenti).** I job branch restano append-only e non fanno mai merge. L'invariante che conta — master si muove solo via close — è intatto.

---

## 5. Sequenza di attuazione candidata

1. `GET /sigiled/contract` + tag di vm-tmpl + pin in `sigiled.toml` (sblocca tutto, costo basso)
2. Log operativo: layer macchina + skeleton nel template + hint di close
3. Auth: provider Authentik `sigiled-*`, dual-auth, actor nei record, morte del bearer
4. Recepimento: sync script + drift detection + `template_behind`
5. Concorrenza: merge-lock, merge al close, merge debt, regole 4/7 nuove

Sequenza alternativa: auth prima, se il dolore della chiave unica in giro per le skill diventa prioritario.

---

## 6. Questioni aperte

1. **Scope auth v1**: solo operatore + agenti LLM; il lattice di gruppi umani (family/friends) arriva al primo use case vero.
2. ~~Il repo di SIGILED stesso è SIGILED-registrato?~~ **Risolta 2026-08-02: sì — §7, DEC-11…15.** La piattaforma v2 si chiama SIGILED; la resurrezione è affidata a `sigiled-supervisor`.
3. Claim di gruppo nei token via property mapping — da verificare su Authentik 2026.5.6.
4. Lista definitiva delle operazioni che richiedono approval (candidati: `projects new`, apps verbs; da ratificare).
5. Soglia di merge debt oltre la quale `status` urla (candidata: 1 — qualunque debt è urlato).
6. Durate definitive dei token (proposte: access driver breve ~1h auto-mintato; approval 12h; refresh 30d).
7. ~~Toolchain del workspace~~ **Risolta 2026-08-02: immagine base per tag + ext per linguaggio — §3.1, DEC-17/18.**

---

## 7. Autogestione — SIGILED è SIGILED-registrato (ratificato 2026-08-02)

La piattaforma si chiama **SIGILED** (nata «SIGILED v2» → «SIGILED», rinominata 2026-08-03): progetto e codice in `ivan-saorin/sigiled`; il contratto di guida resta **SIGILED**. La frase incisa: **SIGILED gestisce tutto di sé tranne la propria resurrezione.**

| Cosa | Dove vive | Chi lo muove |
|---|---|---|
| Codice di SIGILED | repo `ivan-saorin/sigiled` (storia da `ivan-saorin/sigiled`) | sessioni SIGILED, come tutti i progetti |
| Contratto SIGILED | `docs/` di questo repo | sessioni; servito da `GET /sigiled/contract` allo sha deployato |
| Servizio in esecuzione | box, sha pinnato | deploy out-of-band via **sigiled-supervisor** — mai SIGILED su SIGILED |
| Stato runtime | DB sul box (+ backup) | SIGILED; migrazioni expand-contract |
| Resurrezione | `sigiled-supervisor` | API propria: chiamarla restarta sigiled |

Regole specifiche:

- **sigiled-supervisor**: supervisor esterno minimale (~100 righe — se cresce, sta sbagliando), repo proprio `ivan-saorin/sigiled-supervisor`, deploy indipendente da SIGILED (sul box, **mai come `[app]` di SIGILED**: è nel percorso di resurrezione, non può dipendere da ciò che resuscita). Espone una sua API: chiamarla = restart di sigiled (pull allo sha pinnato → build → restart → health check → report). Endpoint protetto e loggato; deve restare raggiungibile a stack mezzo morto, quindi auth propria e semplice, non OIDC.
- **Bootstrap di piattaforma**: progetti creati freschi, il codice entra via prima sessione — si aggira il 503 di session-start sui progetti adottati (bug noto da sistemare).
- **Approval obbligatoria**: sessioni su `sigiled` e `sigiled-supervisor` richiedono approval valida (DEC-02/03) — il repo che governa tutti i repo richiede l'operatore presente.
- **Mai auto-deploy al close**: master si muove via close; il deploy resta atto separato e umano.
- **Runbook di rollback** in `docs/runbook-deploy.md` — leggibile da GitHub anche a SIGILED morto; da scrivere insieme al primo deploy.

---

## 8. Registro delle decisioni

| # | Decisione |
|---|---|
| DEC-01 | Auth a due gambe: `client_credentials` per-driver (macchina) + device flow con approval umana (umano). Il bearer unico muore dopo una finestra dual-auth. **Ratificata 2026-08-03.** |
| DEC-02 | Custodia dei token umani **lato SIGILED** (DB + auto-refresh serializzato); mai nelle skill, mai nei transcript. Nelle skill solo `client_id`/`client_secret` del driver. **Ratificata 2026-08-03.** |
| DEC-03 | `actor` a due componenti `{driver, approval}` su sessioni e job; capability map SIGILED-locale secondo la dottrina «IdP membership-only». **Ratificata 2026-08-03.** |
| DEC-04 | Log operativo a due layer: macchina via API dal DB di SIGILED; narrativo `docs/log-operativo.md` dal template, project-owned per sempre. Regola SIGILED: chiudi lavoro coerente → voce in cima. **Ratificata 2026-08-03.** |
| DEC-05 | Template versioning con pin `template = "vm-tmpl@x.y.z"` in `sigiled.toml`, recepimento on-demand con drift detection; **mai auto-update**. **Ratificata 2026-08-03.** |
| DEC-06 | Il motore serve il proprio contratto: `GET /sigiled/contract`, versionato; le skill si auto-verificano contro `healthz.version`. **Ratificata 2026-08-03.** |
| DEC-07 | Concorrenza a riconciliazione: un branch per workload, lock solo nella sezione critica di merge; niente più 409 su open. **Ratificata 2026-08-03.** |
| DEC-08 | Merge debt con pacchetto di contesto; risoluzione **obbligatoria prima di qualsiasi altro lavoro, quale che sia il modello**; se incerto, chiedi all'operatore. **Ratificata 2026-08-03.** |
| DEC-09 | Merge commit, non rebase: la traccia del confine di sessione è memoria. **Ratificata 2026-08-03.** |
| DEC-10 | Conflitti semantici: dopo merge multipli recenti, compilazione di scrupolo obbligatoria; se rotta, DEVE essere sistemata prima di procedere. **Ratificata 2026-08-03.** |
| DEC-11 | SIGILED è SIGILED-registrato (autogestione): il codice della piattaforma vive nel repo del progetto; il servizio è una deployment a sha pinnato; SIGILED gestisce tutto di sé tranne la propria resurrezione (§7). **Ratificata 2026-08-02.** |
| DEC-12 | ~~Naming v2: SIGILED~~ **Emendata 2026-08-03: la piattaforma si chiama SIGILED.** Domini `sigiled.dev` + `sigilled.dev` (guardiano ortografico) acquistati dal Re; il progetto continua in `ivan-saorin/sigiled`; questo repo è archiviato come fondazione. |
| DEC-13 | La resurrezione è un servizio: `sigiled-supervisor`, ~100 righe, repo e deploy propri (mai `[app]` di SIGILED), API autonoma con auth semplice — chiamarla restarta sigiled. **Ratificata 2026-08-02.** |
| DEC-14 | Bootstrap dei progetti di piattaforma: creazione fresca, codice via prima sessione (niente adozione; il 503 di session-start su adottati resta bug noto). **Ratificata 2026-08-02.** |
| DEC-15 | Sessioni su `sigiled` (nato `sigiled`) e `sigiled-supervisor` richiedono approval valida: la gamba umana è obbligatoria per il control plane. **Ratificata 2026-08-02; nome progetto aggiornato 2026-08-03.** |
| DEC-16 | Linguaggio del control plane: **Rust** — conferma la realtà esistente (l'agent dei workspace `vm-base` è già un server axum; `ext/` sono crate Rust) e la estende a sigiled e sigiled-supervisor. **Ratificata 2026-08-02.** |
| DEC-17 | Workspace v2 = **immagine base pre-buildata per tag**: `FROM vm-base:x.y.z` + layer di toolchain del progetto. Fine del vendoring di `server/`+`ext/`+`build-ext.sh` nei repo e della ricompilazione a ogni build (§3.1). **Ratificata 2026-08-02.** |
| DEC-18 | Ext per linguaggio: `ext-rust/` (compiled-in, come oggi), `ext-py/`, `ext-go/` come processi locali supervisionati proxati da vm-base; contratto unico HTTP a `/x/<nome>` dentro il token gate (§3.1). **Ratificata 2026-08-02.** |
| DEC-19 | SIGILED v2 sarà **open source al 100%**, con landing GitHub Pages esplicativa. Il repo si scrive da subito come pubblico: igiene dei segreti sulla storia, commit message pubblicabili, stack-specifics estratti in config. Preparazione «flip-ready» nella sessione 1b del build plan. **Ratificata 2026-08-02.** |
| DEC-20 | Le immagini base `vm-base:x.y.z` sono pubblicate **pubbliche** su ghcr (`ghcr.io/ivan-saorin/vm-base`): pull senza credenziali dal box e da chiunque self-hosti; PAT solo per il push. Il pin (template Dockerfile e script) usa il nome completo. Nessun segreto vive nelle immagini per costruzione (regola 8). **Ratificata 2026-08-02.** |
| DEC-21 | Nome pubblico = **sigiled**, confermato dalla verifica collisioni (sessione 1b, 2026-08-02): sulle ricerche web «sigiled» esiste solo come aggettivo (carte Magic, un campione di Raid Shadow Legends, prosa Raku) — nessun progetto software, libreria, azienda o prodotto. Le collisioni pesanti che motivavano la verifica riguardavano il nome di battesimo, già eradicato con DEC-12 emendata. Tagline disambiguante sulla landing; nessuna nuova decisione richiesta al Re — registrazione dell'esito. |
| DEC-22 | La sigla dell'orchestratore v1 **scompare dal repo**: il nome è SIGILED ovunque — verbi a `/sigiled/*` (es. `GET /sigiled/contract`), manifest `sigiled.toml` (la v2 lo legge con fallback sul nome v1 per i repo nati prima), provider OAuth2 `sigiled-*`, design doc `sigiled-v2.md`. Due eccezioni operative, non negoziabili coi fatti: il manifest di **radice** conserva il nome file v1 finché l'orchestratore v1 costruisce le sessioni di questo repo (cade al cutover), e `mgr-smoke` resta nel registry v1 come lapide (nessun delete verb). La v1 in produzione risponde a `/mgr` fino al cutover: fuori repo, non rinominabile da qui. **Ratificata 2026-08-02 (in chat).** |
| DEC-24 | La skill per-istanza del driver è **generata, mai editata a mano**: `GET /sigiled/skill/{driver}` renderizza `docs/skill-template.md` coi valori dell'istanza; il `client_secret` arriva vivo dall'API admin dell'IdP quando `AUTHENTIK_API_TOKEN` è configurato, placeholder altrimenti. Gated da approval (riga di capability accanto a projects-new): la risposta può portare una credenziale. Richiesta dal Re e registrata in chat, 2026-08-03. |
| DEC-23 | Licenza: **MIT** (`LICENSE` alla radice, campo `license` nei crate). Proposta Apache-2.0 della sessione 1b scartata dal Re: semplicità sopra il patent grant. **Ratificata 2026-08-02 (in chat).** |
| DEC-25 | **Immagini di sessione per-progetto** — l'hook che la §3.1 prometteva, scoperto mancante il 2026-08-08 (ogni workspace girava sulla vm-base globale; una sessione su questo stesso repo Rust non aveva né cargo né cc). Punto di pin = il manifest, che chiude la scelta aperta della §3.1: `[workspace] dockerfile = "…"` in sigiled.toml — mai una convenzione sul nome file, perché il Dockerfile di radice di questo repo builda vm-base (ruolo publisher, DEC-17) e NON è un'immagine di sessione. Tag `vm-{p}:df-{blob12}`, content-addressed sul blob git del dockerfile a master: i commit che non lo toccano non ribuildano mai, un edit ribuilda alla open successiva (sincrona: la open È l'attesa). Le sessioni ripiegano sulla base **urlando** (`image: {used, requested, build_error}` nella risposta di open/recycle — la sessione è lo strumento di riparazione del proprio dockerfile); i job **falliscono subito** (un batch senza la toolchain dichiarata non deve mentire). Tabella assente = base globale, comportamento precedente. Registrata in sessione f53cbce3, 2026-08-08. |
