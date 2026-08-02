# MGR v2 — Documento di design

**Versione:** 0.2 · **Data:** 2026-08-02 · **Stato:** DEC-11…15 (autogestione) **ratificate dal Re** 2026-08-02; DEC-01…10 restano da ratificare
**Origine:** sessione di design del 2026-08-02 (driver: Kimi K3), partita dalla lettura degli aggiornamenti di `tomes-and-tales` (auth deployata con Authentik) e `torchio` (requisiti v0.3, DEC-17/18): i pattern nati nei progetti **salgano di un livello**, dentro la piattaforma.
**Memoria operativa:** `docs/log-operativo.md` — ogni sessione la aggiorna (convenzione §3).

---

## 0. Sintesi

MGR v2 sono **quattro cambiamenti** sorretti da un solo principio: **git è il modello mentale di tutto lo stack**, non solo dei repo.

1. **Auth a due gambe** — i driver LLM diventano client OAuth2 di Authentik (`client_credentials`, identità macchina per-driver); l'approvazione umana arriva via *device flow* con token custoditi **lato MGR**, mai nelle skill. Il bearer unico muore.
2. **Log operativo a due layer** — MGR espone la storia meccanica dal proprio DB (`GET /mgr/projects/{p}/log`); il file narrativo `docs/log-operativo.md` nasce dal template e resta per sempre del progetto.
3. **Template versioning** — il recepimento (torchio DEC-17) sale di livello: vm-tmpl versionato con tag, pin in `mgr.toml`, sync on-demand con drift detection, **mai auto-update**. Il motore serve il proprio contratto (`GET /mgr/contract`).
4. **Concorrenza a riconciliazione** — addio lock di sessione: un branch per workload, merge al close, e un **merge debt** con pacchetto di contesto che la sessione successiva **deve** risolvere prima di qualsiasi altro lavoro, quale che sia il modello che la guida.

I quattro si compongono: l'auth dà autori veri al log; il log registra update di template e merge debt; il recepimento distribuisce il contratto nuovo ai progetti; la concorrenza rende tutto multi-driver senza serrature.

---

## 1. Auth — due gambe

### 1.1 La dottrina, generalizzata da tnt

Come in tnt (docs/auth.md §1): **l'IdP sa solo membership; ogni applicazione mappa gruppi → capability localmente**. Authentik è l'IdP dello stack, già live su `auth.016180.xyz`. MGR diventa una relying party come le altre — niente IdP privato, niente console nuova: Authentik È la console.

### 1.2 Evidenza raccolta dal vivo (2026-08-02)

Verificato con sonde anonime:

- `GET /application/o/tomes-and-tales/.well-known/openid-configuration` → **200**, discovery completa e pubblica (OIDC nasce per essere scopribile).
- Grant dichiarati: `authorization_code`, `refresh_token`, `implicit`, **`client_credentials`**, `password`, **`urn:ietf:params:oauth:grant-type:device_code`** (con `device_authorization_endpoint` dedicato).
- `POST /application/o/token/` senza credenziali → `400 invalid_client`: il token endpoint è vivo e sorvegliato; la porta per le macchine esiste già.
- `POST /application/o/device/` con il solo `client_id` pubblico di tnt → `400 invalid_client`: il device endpoint è attivo; serve un provider configurato per device flow.
- JWKS del provider tnt = `{}` perché firma **HS256** (simmetrico). Per i provider MGR: **RS256**, così MGR valida i JWT localmente via JWKS.

### 1.3 Gamba machine — client_credentials per driver

- **Un provider OAuth2 per driver** (`mgr-kimi`, `mgr-claude`, …): ciascuno con la propria coppia `client_id`/`client_secret`, grant ristretto a `client_credentials`, firma RS256. In Authentik un provider = una coppia → per-driver significa revoca per-driver e audit per-driver.
- **Il driver si auto-minta i token**: `POST /application/o/token/` → access token breve → `Authorization: Bearer` verso `api.016180.xyz`. Scaduto → se ne prende un altro. **Zero umani nel loop dopo il setup.**
- **Concorrenza sicura per costruzione**: ogni token è indipendente, nessuno stato mutabile condiviso fra chat parallele — il problema di rotazione dei refresh token (§1.5) qui non esiste.
- **Nella skill SEAL**: `client_id` + `client_secret` al posto del bearer monolitico. Stesse regole di handling (mai echo, mai commit), stessa storia di rotazione (401 → chiedi il nuovo valore). Un leak compra solo token brevi di *quel* driver, revocabile in un click.

### 1.4 Gamba umana — device flow, custodia lato MGR

Per le operazioni che devono portare «Ivan ha approvato»:

```
1. driver → POST /mgr/auth/elevate
2. MGR (client OIDC) → POST auth.016180.xyz/application/o/device/   [provider mgr-device]
3. MGR → driver → chat: «vai su auth.016180.xyz/device, codice ABCD-1234»
4. operatore approva dal browser (una volta)
5. MGR fa polling al token endpoint, ottiene access+refresh token
   → li custodisce nel proprio DB, auto-refresh serializzato nel suo processo
```

- **Access token 12h, refresh 30 giorni** (configurazione provider): l'umano approva ~**una volta al mese per driver**, non 1-2 volte al giorno.
- **Il token non tocca mai né skill né PC né transcript**: MGR è l'unico componente con stato persistente e processo sempre vivo — il custode naturale. Niente corse di rotazione: la serializzazione è interna a MGR.
- Stampare `user_code` in chat è sicuro per costruzione: autorizza soltanto; i token arrivano a chi detiene il `device_code`, che resta server-side.

### 1.5 Perché non «token di lunga vita nella skill»

Ipotesi valutata e scartata (device flow + auto-refresh con refresh token salvato nella skill):

1. **Corse di rotazione** — Authentik ruota il refresh token a ogni uso; più chat parallele con lo stesso token si invalidano a vicenda, e il problema rientra dalla finestra proprio durante le sessioni parallele.
2. **Leak surface** — la skill viene letta per intero nel contesto a ogni attivazione: un segreto da 30 giorni in ogni transcript di ogni provider.
3. **File ≠ database** — la skill è un documento condiviso fra superfici; farla riscrivere a ogni refresh è fragile o impossibile (alcune superfici non possono scriverla).

### 1.6 Actor e capability

Ogni record session/job guadagna:

```
actor: { driver: "mgr-kimi", approval: "ivan (device, scade 2026-08-03T01:00)" | null }
```

Capability map v1 (MGR-locale):

| | `stack:admins` | `stack:drivers` |
|---|---|---|
| sessions open/close/recycle, git, exec | ✓ | ✓ |
| jobs run/recap | ✓ | ✓ |
| projects new | ✓ | con approval |
| apps verbs (start/stop/restart/upgrade) | ✓ | con approval |

### 1.7 Migrazione dal bearer unico

Finestra **dual-auth**: l'edge accetta legacy bearer (= bootstrap admin) e token Authentik; si creano i provider `mgr-*`, si aggiornano le skill, poi il legacy muore. La skill gestisce già la rotazione via 401: storia compatibile.

### 1.8 Note di sicurezza

- Grant `password` **disabilitato** sui provider MGR — solo `client_credentials` (driver) e `device_code` (mgr-device).
- Secret driver lunghi e generati; verificare rate-limiting all'edge sulla route del token endpoint.
- Validazione in MGR: **JWKS + RS256** (locale, niente chiamata per richiesta); introspezione come fallback/debug.
- Claim di gruppo nei token via property mapping Authentik — da verificare in configurazione (serve per `stack:drivers` come claim).

---

## 2. Log operativo — due layer

- **Layer macchina (MGR-owned)**: MGR ha già tutti i dati (sessioni, close con esito merge, job run, build app) nel proprio DB. Li espone: **`GET /mgr/projects/{p}/log`**. Zero scritture nei repo dei progetti, zero violazioni della proprietà dei contenuti.
- **Layer narrativo (driver-owned)**: `docs/log-operativo.md` con contratto in testa (tre domande: dove eravamo / dove prevedevamo di andare / cosa è stato fatto + scarti, stato, prossimo passo; voci non si cancellano, si correggono con voci nuove). **Lo skeleton nasce dal template alla creazione del progetto e da quel momento è del progetto, per sempre** — la regola torchio DEC-18 generalizzata. Il template non lo tocca mai dopo la creazione: collisione risolta per costruzione.
- **Regola SEAL nuova**: *chiudi lavoro coerente → aggiungi una voce in cima al log operativo*.
- **Hint onesto**: `close` risponde con `log_operativo_touched: false` quando il file non è stato modificato — specchio, non enforcement.

## 3. Template versioning — il recepimento sale di livello

La meccanica di torchio DEC-17, applicata a vm-tmpl:

- **vm-tmpl versionato**: tag semver + CHANGELOG.
- **Pin alla creazione**: `mgr.toml` on master guadagna `template = "vm-tmpl@x.y.z"` — casa naturale, è già il file che MGR legge.
- **Recepimento**: allowlist di path MGR-owned + `sync` script + **drift detection** (se hai toccato file MGR-owned, si ferma e segnala). Rollback = `git revert` o re-pin al tag precedente.
- **Mai auto-update** — simmetria con torchio DEC-07: MGR non riscrive mai i repo dei progetti di propria iniziativa. Sync v1 in sessione (close = imbragatura), v2 come job.
- **Visibilità**: `status` mostra `template_behind: true` accanto a `needs_merge`.
- **Il contratto servito dal motore**: **`GET /mgr/contract`** — il testo SEAL canonico, versionato. Con `healthz.version` già esistente, ogni driver può verificare la freschezza della propria skill e rigenerarla (l'Appendix A lo prescrive; ora diventa meccanico).

## 4. Concorrenza — da esclusione a riconciliazione

### 4.1 Il cambio strutturale

Il lock non sparisce: **si restringe da tutta la vita della sessione a una sezione critica di pochi secondi al merge**. Si paga il costo della coordinazione solo quando i conflitti esistono davvero, e git è costruito per minimizzarli.

1. `open` → branch `session/{id}` da master corrente. **Niente più 409**: N sessioni concorrenti, N container isolati, N token per-sessione (il modello a token già lo supporta, non cambia nulla).
2. Lavoro, commit, push automatico — identico a oggi.
3. `close` → MGR acquisisce il merge-lock del progetto (secondi) e tenta in sequenza: **fast-forward** (master fermo: caso comune) → **merge a tre vie** (master mosso ma cambi disgiunti: git fonde da solo) → **conflitto**.
4. Close simultanee: la sezione critica le serializza — una vince, l'altra vede master mosso e va nel ramo merge.

### 4.2 Merge debt

Al conflitto: master resta dov'è, il branch resta, e MGR registra il pacchetto di contesto — perché chi risolverà **non ha scritto nessuna delle due metà**:

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

Futuro (gancio, non v1): hook post-merge opzionale dichiarato in `mgr.toml` per automatizzare il check.

### 4.4 Le regole riscritte

- **Regola 4** — da «un workload per progetto» a: **un branch per workload; master è l'arbitro al close.** Il 409 sulle sessioni sparisce; resta il divieto di retry-hammer su lock di merge.
- **Regola 7** — da «master si muove solo via close (fast-forward)» a: **master si muove solo via close (FF preferito, merge altrimenti).** I job branch restano append-only e non fanno mai merge. L'invariante che conta — master si muove solo via close — è intatto.

---

## 5. Sequenza di attuazione candidata

1. `GET /mgr/contract` + tag di vm-tmpl + pin in `mgr.toml` (sblocca tutto, costo basso)
2. Log operativo: layer macchina + skeleton nel template + hint di close
3. Auth: provider Authentik `mgr-*`, dual-auth, actor nei record, morte del bearer
4. Recepimento: sync script + drift detection + `template_behind`
5. Concorrenza: merge-lock, merge al close, merge debt, regole 4/7 nuove

Sequenza alternativa: auth prima, se il dolore della chiave unica in giro per le skill diventa prioritario.

---

## 6. Questioni aperte

1. **Scope auth v1**: solo operatore + agenti LLM; il lattice di gruppi umani (family/friends) arriva al primo use case vero.
2. ~~Il repo di MGR stesso è MGR-registrato?~~ **Risolta 2026-08-02: sì — §7, DEC-11…15.** La piattaforma v2 si chiama SEAL; la resurrezione è affidata a `seal-supervisor`.
3. Claim di gruppo nei token via property mapping — da verificare su Authentik 2026.5.6.
4. Lista definitiva delle operazioni che richiedono approval (candidati: `projects new`, apps verbs; da ratificare).
5. Soglia di merge debt oltre la quale `status` urla (candidata: 1 — qualunque debt è urlato).
6. Durate definitive dei token (proposte: access driver breve ~1h auto-mintato; approval 12h; refresh 30d).

---

## 7. Autogestione — MGR è MGR-registrato (ratificato 2026-08-02)

La piattaforma v2 si chiama **SEAL**: contratto, progetto e codice coincidono in `ivan-saorin/seal` (questo repo). La frase incisa: **SEAL gestisce tutto di sé tranne la propria resurrezione.**

| Cosa | Dove vive | Chi lo muove |
|---|---|---|
| Codice di SEAL | repo `ivan-saorin/seal` | sessioni SEAL, come tutti i progetti |
| Contratto SEAL | `docs/` di questo repo | sessioni; servito da `GET /mgr/contract` allo sha deployato |
| Servizio in esecuzione | box, sha pinnato | deploy out-of-band via **seal-supervisor** — mai SEAL su SEAL |
| Stato runtime | DB sul box (+ backup) | SEAL; migrazioni expand-contract |
| Resurrezione | `seal-supervisor` | API propria: chiamarla restarta seal |

Regole specifiche:

- **seal-supervisor**: supervisor esterno minimale (~100 righe — se cresce, sta sbagliando), repo proprio `ivan-saorin/seal-supervisor`, deploy indipendente da SEAL (sul box, **mai come `[app]` di MGR**: è nel percorso di resurrezione, non può dipendere da ciò che resuscita). Espone una sua API: chiamarla = restart di seal (pull allo sha pinnato → build → restart → health check → report). Endpoint protetto e loggato; deve restare raggiungibile a stack mezzo morto, quindi auth propria e semplice, non OIDC.
- **Bootstrap di piattaforma**: progetti creati freschi, il codice entra via prima sessione — si aggira il 503 di session-start sui progetti adottati (bug noto da sistemare).
- **Approval obbligatoria**: sessioni su `seal` e `seal-supervisor` richiedono approval valida (DEC-02/03) — il repo che governa tutti i repo richiede l'operatore presente.
- **Mai auto-deploy al close**: master si muove via close; il deploy resta atto separato e umano.
- **Runbook di rollback** in `docs/runbook-deploy.md` — leggibile da GitHub anche a SEAL morto; da scrivere insieme al primo deploy.

---

## 8. Registro delle decisioni

| # | Decisione |
|---|---|
| DEC-01 | Auth a due gambe: `client_credentials` per-driver (macchina) + device flow con approval umana (umano). Il bearer unico muore dopo una finestra dual-auth. |
| DEC-02 | Custodia dei token umani **lato MGR** (DB + auto-refresh serializzato); mai nelle skill, mai nei transcript. Nelle skill solo `client_id`/`client_secret` del driver. |
| DEC-03 | `actor` a due componenti `{driver, approval}` su sessioni e job; capability map MGR-locale secondo la dottrina «IdP membership-only». |
| DEC-04 | Log operativo a due layer: macchina via API dal DB di MGR; narrativo `docs/log-operativo.md` dal template, project-owned per sempre. Regola SEAL: chiudi lavoro coerente → voce in cima. |
| DEC-05 | Template versioning con pin `template = "vm-tmpl@x.y.z"` in `mgr.toml`, recepimento on-demand con drift detection; **mai auto-update**. |
| DEC-06 | Il motore serve il proprio contratto: `GET /mgr/contract`, versionato; le skill si auto-verificano contro `healthz.version`. |
| DEC-07 | Concorrenza a riconciliazione: un branch per workload, lock solo nella sezione critica di merge; niente più 409 su open. |
| DEC-08 | Merge debt con pacchetto di contesto; risoluzione **obbligatoria prima di qualsiasi altro lavoro, quale che sia il modello**; se incerto, chiedi all'operatore. |
| DEC-09 | Merge commit, non rebase: la traccia del confine di sessione è memoria. |
| DEC-10 | Conflitti semantici: dopo merge multipli recenti, compilazione di scrupolo obbligatoria; se rotta, DEVE essere sistemata prima di procedere. |
| DEC-11 | MGR è MGR-registrato (autogestione): il codice della piattaforma vive nel repo del progetto; il servizio è una deployment a sha pinnato; SEAL gestisce tutto di sé tranne la propria resurrezione (§7). **Ratificata 2026-08-02.** |
| DEC-12 | Naming v2: la piattaforma si chiama **SEAL** — contratto, progetto e repo coincidono (`ivan-saorin/seal`). **Ratificata 2026-08-02.** |
| DEC-13 | La resurrezione è un servizio: `seal-supervisor`, ~100 righe, repo e deploy propri (mai `[app]` di MGR), API autonoma con auth semplice — chiamarla restarta seal. **Ratificata 2026-08-02.** |
| DEC-14 | Bootstrap dei progetti di piattaforma: creazione fresca, codice via prima sessione (niente adozione; il 503 di session-start su adottati resta bug noto). **Ratificata 2026-08-02.** |
| DEC-15 | Sessioni su `seal` e `seal-supervisor` richiedono approval valida: la gamba umana è obbligatoria per il control plane. **Ratificata 2026-08-02.** |
