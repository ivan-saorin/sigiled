# SIGILED v2 — Piano di costruzione in 4 sessioni

**Versione:** 0.1 · **Data:** 2026-08-02 · **Driver previsto:** Claude Code (vale per qualunque driver)
**Fonte di verità:** `docs/mgr-v2.md` (design + DEC-01…18). Questo piano è esecuzione; in caso di conflitto vince il design doc.

**Nota 2026-08-03:** la piattaforma si chiama **SIGILED** (DEC-12 emendata); le sessioni di costruzione si aprono sul progetto `sigiled` — la storia del repo `sigiled` viaggia là col push dell'operatore. DEC-19/20 (open source, ghcr) ratificate nel frattempo; la «sessione 1b» qui sotto è stata aggiunta da Claude Code.

---

## Come si usa questo piano

- **Una sessione = una sezione (§2…§5).** Ogni sessione: open su `sigiled` (nato `sigiled`) → git log → leggi `docs/mgr-v2.md` + questo piano + `docs/log-operativo.md` → lavoro → commit intent-carrying a ogni passo coerente → voce nel log operativo → **close**. Mai lasciare sessioni aperte.
- **Master chiude sempre verde**: build + test passano a fine sessione.
- **Rust ovunque** (DEC-16). Dipendenze minime e motivate nel commit message.
- **Il workspace non ha docker/ssh**: dentro la sessione build, unit test, mock. Deploy e smoke sul box sono dell'operatore — lascia istruzioni precise nel log.
- **Segreti mai in git**; arrivano come env (regola 8 del contratto).
- **Se una DEC non regge al codice**: non cambiarla in silenzio — voce di log + proposta di emendamento in `mgr-v2.md`; il Re ratifica.
- **Se sfori la sessione**: commit wip onesto + voce «in corso» nel log; la sessione successiva ricalibra, registrando lo scarto.

---

## 1. Prerequisiti (operatore, prima della sessione 1)

1. **Codice MGR attuale**: push su `ivan-saorin/sigiled` master, oppure decisione esplicita di greenfield. Se il codice attuale non è Rust, DEC-16 implica **rewrite guidata** — il contratto SIGILED v1 (la skill, ex SIGILED) è la spec completa del comportamento v1.
2. **Token API Authentik** in stack env (serve alla sessione 3); in alternativa l'operatore crea i provider a mano con le istruzioni che la sessione 3 lascerà.
3. **Registry immagini** per `vm-base:x.y.z` raggiungibile dal box (ghcr.io o registry di stack).
4. Le sessioni su `sigiled` girano su MGR **v1** col bearer legacy finché la dual-auth non esiste: normale amministrazione, la v2 si costruisce da dentro la v1.

---

## 2. Sessione 1 — fondamenta: repo, dominio, contratto, immagine base

**Deliverables:**

- **Assessment iniziale** (prima cosa, nel log): evoluzione del codice importato vs greenfield. Motivata in cinque righe.
- **Layout repo**: `sigiledd/` (orchestratore), `vm-base/` (agent: port di vm-tmpl `server/`+`build-ext.sh`, convenzione `ext-rust/`), `template/` (vm-tmpl v2: Dockerfile `FROM vm-base` + `docs/` skeleton con log-operativo + `mgr.toml` commentato + `ext-rust/` vuota di esempio).
- **`GET /healthz`** `{status, version}` e **`GET /mgr/contract`**: serve il contratto canonico dal repo allo sha deployato. Include **scrivere `docs/sigiled-contract.md`** — il contratto v2, generato da `mgr-v2.md` (regole nuove: concorrenza, merge debt, log operativo, auth a due gambe).
- **Parse di `template = "vm-tmpl@x.y.z"`** in `mgr.toml`: il project record guadagna `template_version`, esposto in `GET /mgr/projects`.
- **Script immagine base** (`images/build-vm-base.sh`): build locale di `vm-base:0.1.0`; push = operatore se il registry non è raggiungibile dal workspace.

**Acceptance:** cargo build verde; unit test del dominio verdi; `/mgr/contract` risponde in run locale; voce di log con assessment e scarti; close.

## 2b. Sessione 1b — open source al 100% (DEC-19/20)

Il Re ha deciso (2026-08-02, in chat, ratifica diretta): SIGILED v2 sarà **open
source al 100%**, con una landing GitHub Pages che spiega il progetto. Il repo
si scrive da subito come se fosse pubblico. Questa sessione rende il repo
«flip-ready»: pubblicarlo deve ridursi a girare l'interruttore di visibilità.

**Deliverables:**

- **LICENSE**: proposta **Apache-2.0** (patent grant, standard per
  infrastruttura); il Re ratifica la scelta in sessione — se preferisce MIT,
  è un file diverso, zero impatti.
- **Verifica naming**: le collisioni che motivavano questa voce riguardavano
  il nome di battesimo della piattaforma (metodo MIT di self-adapting LLMs,
  libreria crypto Microsoft omonima…), eradicato con DEC-12 emendata. Resta
  da verificare che **sigiled** sia pulito e registrare l'esito in DEC. La
  landing e il README usano il nome verificato con tagline disambiguante.
- **Audit igiene della storia git**: scan di tutta la storia (bearer, token,
  chiavi, path privati) — la storia è giovane, farlo ORA che riscriverla è
  ancora possibile senza dolore. Regola permanente da qui in poi: ogni
  commit si scrive sapendo che sarà pubblico.
- **Generalizzazione stack-specifics**: inventario dei punti dove lo stack
  di Ivan è cablato (api/auth/search.016180.xyz, `ghcr.io/ivan-saorin`,
  nomi provider `mgr-*`, `automa`) → in sigiledd diventano config/env con
  default documentati; i docs di design restano liberi di citare lo stack
  di riferimento come istanza esemplare.
- **ghcr pubblico (DEC-20)**: `template/Dockerfile` → `FROM
  ghcr.io/ivan-saorin/vm-base:0.1.0`; `images/build-vm-base.sh` con lo
  stesso default per `VM_BASE_REGISTRY`; istruzioni per marcare il package
  public in `docs/runbook-deploy.md` (bozza).
- **README pubblico (EN) + landing gh-pages**: `docs/landing/` con la pagina
  che spiega mental model, il contratto come prodotto (`GET /mgr/contract`),
  concorrenza a riconciliazione, quickstart self-host, modello di sicurezza.
  Pages si attiva solo al flip; qui si prepara tutto. I doc interni restano
  in italiano (memoria di progetto), con nota esplicita nel README.

**Acceptance:** LICENSE presente; audit storia documentato pulito nel log;
FROM/script coerenti su ghcr; landing renderizzabile; naming registrato in
DEC; build+test verdi; close.

## 3. Sessione 2 — log operativo macchina + recepimento

**Deliverables:**

- **`GET /mgr/projects/{p}/log`**: storia meccanica dal DB (sessioni, close con esito merge, job run) — JSON + render markdown.
- **Hint di close**: `log_operativo_touched: true|false` nella risposta di close.
- **`template/tools/sync-template.sh`**: vendored-replace su allowlist + pin a tag + **drift detection** (si ferma e segnala se i path template-owned hanno modifiche locali).
- **`template_behind`** in `GET /mgr/projects` e in `status`.
- **Banco di prova**: il progetto `mgr-smoke` esiste già — usalo per il test end-to-end del sync (induci drift, verifica che si fermi e segnali).

**Acceptance:** sync su `mgr-smoke` con drift indotto → stop + segnalazione; sync pulito → commit + `TEMPLATE_VERSION` aggiornato; log macchina leggibile via API; close.

## 4. Sessione 3 — auth a due gambe

**Deliverables:**

- **Middleware dual-auth**: accetta legacy bearer (→ bootstrap admin) **o** JWT Authentik (JWKS RS256 con cache; introspezione come fallback).
- **`actor` {driver, approval}** su sessioni e job; **capability map v1**: `stack:admins` / `stack:drivers`; approval richiesta per `projects new`, apps verbs, e **sessioni su `sigiled` e `sigiled-supervisor`** (DEC-15).
- **`POST /mgr/auth/elevate`**: device flow via provider `mgr-device`; token custodito nel DB, refresh serializzato; `GET /mgr/auth/approvals` per ispezione.
- **Provisioning dei provider** `mgr-device` + `mgr-<driver>`: via API Authentik col token da stack env, oppure istruzioni passo-passo per l'operatore (script `docs/authentik-setup.md`).
- **Nota di migrazione per le skill** (`docs/skill-migration.md`): come un driver passa da bearer legacy a `client_id`/`client_secret` — testo pronto da incollare nelle skill.

**Acceptance:** con JWT driver valido → open/close normali; senza approval → open su `sigiled` negato; con approval device approvata dall'operatore → passa; legacy bearer ancora funzionante (finestra dual-auth); close.

## 5. Sessione 4 — concorrenza + supervisor + runbook

**Deliverables:**

- **Niente lock di sessione**: open sempre ammesso; **merge-lock** per progetto (sezione critica di secondi).
- **Close**: FF → merge a tre vie → **merge debt** con pacchetto `{branch, conflicted_files, ours/theirs + commit messages, since}`; `open` espone `merge_debt` in cima; `status` mostra la coda. Aggiorna `docs/sigiled-contract.md` (regole 4/7 nuove, protocollo di risoluzione, compilazione di scrupolo DEC-10).
- **`sigiled-supervisor`** (sessione sull'altro progetto): Rust ~100 righe — `GET /health`, `GET /sigiled/status`, `POST /sigiled/restart {sha?}` (sha = rollback), log append-only, bearer statico da env. Secondo i suoi `docs/requisiti.md`.
- **`docs/runbook-deploy.md`** in sigiled: deploy via supervisor, rollback via sha, recovery a stack morto (SSH + passi manuali).
- **Test di concorrenza su `mgr-smoke`**: due sessioni parallele; merge pulito nel caso disgiunto; conflitto indotto → debt → la sessione seguente risolve col protocollo.

**Acceptance:** test di concorrenza passato e raccontato nel log; supervisor compila e risponde in run locale; runbook presente; close.

---

## 6. Cutover (operatore + Re, dopo la sessione 4 — fuori sessione)

1. Deploy v2 sul box via supervisor, porta/vhost separato (**canary**).
2. **Import del registry v1** (progetti, lock, storia) nel DB v2 — script one-shot da scrivere in sessione dedicata se il volume lo richiede.
3. Migrazione skill dei driver al nuovo auth (testo pronto dalla sessione 3), spegnimento bearer legacy.
4. Switch edge api.016180.xyz → v2; v1 spento ma rollbackabile (sha pinnato al supervisor).
5. Voce «cutover» nel log + aggiornamento `mgr-v2.md` §5 (sequenza → fatto).
