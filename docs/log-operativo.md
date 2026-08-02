# SEAL — Log operativo

**Contratto.** Questo file è la memoria operativa del progetto. Ogni sessione che chiude lavoro coerente **aggiunge una voce in cima** (la più recente prima) e **aggiorna lo Stato attuale**. Ogni voce risponde a tre domande: **dove eravamo**, **dove prevedevamo di andare**, **cosa è stato fatto** — più gli **scarti** fra previsione e fatto, lo **stato a fine sessione** e il **prossimo passo previsto**. I commit message sono la memoria fine; questo log è la memoria grossa. Le voci non si cancellano: si correggono con voci nuove.

**Nota (MGR v2 DEC-04):** questo file è project-owned — nasce dal template alla creazione e il template non lo tocca mai più. Il layer macchina della storia (sessioni, merge, job) sarà esposto da MGR via `GET /mgr/projects/{p}/log`, senza scrivere nei repo.

---

## Stato attuale

_aggiornato: 2026-08-02, sessione 03fe1b7d (sessione 1/4 del build plan)_

- **Su master**: docs (`mgr-v2.md` v0.2, `v2-build-plan.md`, `seal-contract.md` 2.0.0-draft, questo log) **+ codice sessione 1**: workspace cargo con `seald/` (healthz, `GET /mgr/contract`, `GET /mgr/projects` con `template_version`; 7 unit test) e `vm-base/` (port del server v1, build-ext.sh su `ext-rust/`), `template/` (vm-tmpl v2), `images/build-vm-base.sh`, `tools/dev-toolchain.sh`.
- **Decisioni**: DEC-01…DEC-18 registrate. **DEC-11…18 ratificate dal Re 2026-08-02**; DEC-01…10 restano da ratificare — `seal-contract.md` è marcato draft finché non lo sono.
- **Progetti di piattaforma**: `seal` (codice + contratto, questo repo), `seal-supervisor` (resurrezione; spec in `docs/requisiti.md` lì).
- **Per l'operatore**: (a) l'immagine base non è ancora né buildata né pushata — decidere il registry (prerequisito §1.3 del piano) e lanciare `images/build-vm-base.sh` sul box; (b) per la sessione 3 serve il token API Authentik in stack env (prerequisito §1.2).
- **Prossimo passo previsto**: **sessione 1b** (open source flip-ready, DEC-19/20: license, audit storia, naming, ghcr pubblico, landing gh-pages), poi sessione 2/4 — log operativo macchina, hint di close, sync-template con drift detection, `template_behind`; banco di prova `mgr-smoke`.

---

## Voci

### 2026-08-02 · DEC-19/20 ratificate: open source al 100%, immagini pubbliche su ghcr; nasce la sessione 1b — driver: Claude Code (sessione b5245bba)

- **Dove eravamo**: sessione 1/4 chiusa in giornata; in chat, discussione su dove pubblicare le immagini base (prerequisito §1.3 del piano).
- **Fatto**: il Re ha ratificato in chat: (1) **DEC-19** — SEAL v2 sarà open source al 100% con landing GitHub Pages; il repo si scrive da subito come pubblico; (2) **DEC-20** — immagini `vm-base:x.y.z` pubbliche su `ghcr.io/ivan-saorin` (pull senza credenziali, PAT solo per il push, nessun segreto nelle immagini per costruzione). Scritta la **sessione 1b** nel build plan (§2b): license (proposta Apache-2.0), audit igiene della storia git, verifica naming (SEAL collide con altri progetti), generalizzazione degli stack-specifics in config, FROM/script su ghcr, README EN + landing.
- **Scarti**: la decisione sul registry, lasciata all'operatore dalla sessione 1, si è risolta in chat ed è diventata più grande: non «quale registry» ma «il progetto è pubblico». Tentato l'accesso SSH al box dal PC dell'operatore per la build di vm-base: chiave non autorizzata (e host .205 con host key cambiata — segnalato); la build resta all'operatore con le istruzioni lasciate in chat.
- **Stato a fine sessione**: vedi «Stato attuale».
- **Prossimo passo previsto**: sessione 1b, poi sessione 2/4.

### 2026-08-02 · sessione 1/4 chiusa: fondamenta v2 su master — driver: Claude Code (sessione 03fe1b7d)

- **Dove eravamo**: solo docs su master; assessment appena registrato (voce sotto): greenfield per seald, evoluzione per vm-base.
- **Previsione**: i deliverable di §2 del piano — layout, healthz+contract, parse del pin template, template v2, script immagine.
- **Fatto**: tutto §2. (1) Layout: workspace cargo, `server/`→`vm-base/` via git mv, `build-ext.sh` su convenzione `ext-rust/`, Dockerfile root a doppio ruolo (compat sessioni MGR v1 + sorgente di `vm-base:x.y.z`). (2) `docs/seal-contract.md` 2.0.0-draft generato da mgr-v2.md, embedded in seald a compile time (`GET /mgr/contract` serve allo sha del build per costruzione). (3) seald: `/healthz`, `/mgr/contract`, `/mgr/projects` con `template_version` dal pin `template = "vm-tmpl@x.y.z"` (parser con 7 unit test, incluso il test che il manifest del template non regredisca). (4) `template/`: Dockerfile sottile FROM vm-base:0.1.0, mgr.toml commentato col pin, skeleton log-operativo, ext-rust/ vuota. (5) `images/build-vm-base.sh` (versione da vm-base/Cargo.toml). Acceptance verificata: build+test verdi, run locale su :8099 risponde su tutti e tre gli endpoint.
- **Scarti**: uno solo, strumentale — il container di sessione (runtime v1, debian-slim) non ha né rust né cc: per «build e test dentro la sessione» è nato `tools/dev-toolchain.sh` (rustup + gcc-14/binutils/libc6-dev estratti da deb.debian.org in `$HOME/tc` senza root, ~2 min; effimero, da rilanciare dopo recycle). Da valutare in sessione 2 se l'immagine v2 debba includere la toolchain di serie.
- **Stato a fine sessione**: vedi «Stato attuale» sopra. Master chiude verde.
- **Prossimo passo previsto**: sessione 2/4 (log macchina + recepimento); per l'operatore, build+push di vm-base:0.1.0 e registry (§1.3), token Authentik in stack env per la sessione 3 (§1.2).

### 2026-08-02 · sessione 1/4, assessment iniziale: greenfield per seald, evoluzione per vm-base — driver: Claude Code (sessione 03fe1b7d)

- **Dove eravamo**: piano in 4 sessioni su master; il prerequisito §1.1 (push del codice MGR v1 o decisione esplicita di greenfield) non era stato consumato.
- **Assessment** (il bivio del piano, risolto sull'evidenza del repo):
  1. Il codice dell'orchestratore MGR v1 **non è su questo repo**: su master c'è solo il vendoring vm-tmpl v1 (`server/`, `ext/`, `build-ext.sh`, `Dockerfile`, `mgr.toml`) dell'Initial commit.
  2. `seald/` nasce **greenfield guidato**: la spec del comportamento v1 è il contratto SEAL (la skill), la v2 è `docs/mgr-v2.md` — riscrivere in Rust da spec (DEC-16) è più pulito che re-importare un codice che qui non esiste.
  3. `vm-base/` è **evoluzione**: il server axum vendorizzato è già Rust e già in produzione come agent dei workspace — si porta con `git mv` e diventa la sorgente dell'immagine `vm-base:x.y.z` (DEC-17).
  4. `template/` si riscrive sottile per costruzione (DEC-17/18): `FROM vm-base` + skeleton docs + `mgr.toml` commentato + `ext-rust/` d'esempio.
  5. Vincolo pratico: il `Dockerfile` di root deve restare funzionante — MGR **v1** lo usa per costruire l'immagine di sessione di questo stesso repo — quindi si adatta a puntare a `vm-base/`, non si elimina.
- **Prossimo passo previsto**: layout repo, poi seald (healthz + contract + parse template pin), template v2, script immagine. Voce di chiusura a fine sessione.

### 2026-08-02 · piano di costruzione v2 per Claude Code — driver: Kimi K3

- **Dove eravamo**: design completo (DEC-01…18); il Re vuole costruire la v2 in Claude Code, in 3-4 sessioni.
- **Fatto**: scritto `docs/v2-build-plan.md` v0.1 — 4 sessioni (fondamenta+contratto+immagine base; log macchina+recepimento; auth due gambe; concorrenza+supervisor+runbook), prerequisiti operatore, acceptance per sessione, cutover. Il prompt per il driver punta al repo come memoria.
- **Scarti**: il piano prevede esplicitamente il bivio evoluzione/greenfield all'apertura della sessione 1 (il codice MGR attuale non è noto al piano) e l'import del registry v1 al cutover.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: prerequisiti operatore (§1 del piano), poi sessione 1 in Claude Code.

### 2026-08-02 · DEC-17/18: workspace v2 = immagine base + ext per linguaggio — driver: Kimi K3

- **Dove eravamo**: questione aperta §6.7 (toolchain dei workspace); il Re: «per v2, immagine base + /ext-rust; i progetti python si costruiranno /ext-py, quelli go /ext-go».
- **Fatto**: §3.1 nuova (immagine base pre-buildata per tag; `FROM vm-base:x.y.z` + layer progetto; ext-rust compiled-in, ext-py/ext-go come processi supervisionati proxati a `/x/<nome>`; vm-tmpl v2 con allowlist quasi a zero); §6.7 chiusa; DEC-17/18 ratificate. Nomi normalizzati a `ext-<lang>` col trattino.
- **Scarti**: nessuno sulla sostanza; normalizzazione formale dei nomi delle directory.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: costruzione v2 in 4 sessioni da parte di Claude Code, secondo `docs/v2-build-plan.md` (prerequisiti operatore §1 del piano); ratifica DEC-01…10 in parallelo.

### 2026-08-02 · DEC-16: Rust per il control plane; questione toolchain — driver: Kimi K3

- **Dove eravamo**: v0.2 su master; in chat il Re decide il linguaggio di piattaforma.
- **Previsione**: registrare la decisione e verificare i fatti sul template.
- **Fatto**: letti `Dockerfile`, `build-ext.sh`, `mgr.toml`, `server/` del vm-tmpl (via questo repo): l'agent `vm-base` è già Rust/axum, `ext/` sono crate montati a `/x/<nome>` da build-ext.sh, il runtime è debian-slim con git/bash/curl e basta. Registrata DEC-16 (Rust, ratificata); aggiunta questione aperta §6.7 — separazione template-owned/project-owned nel Dockerfile dei workspace (hook toolchain vs immagine base per tag).
- **Scarti**: la domanda del Re («se uno vuole una vm in python?») ha rivelato che oggi ogni progetto **vendorizza e ricompila** l'agent — il recepimento di §3 è ancora più necessario di quanto scritto in v0.1.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica DEC-01…10; la questione toolchain si decide al primo progetto con toolchain non minimale.

### 2026-08-02 · v0.1 → v0.2: autogestione ratificata, nasce seal-supervisor — driver: Kimi K3

- **Dove eravamo**: design v0.1 su master; questione aperta §6.2 — MGR MGR-registrato?
- **Previsione**: discussione in chat su implicazioni e limiti dell'autogestione.
- **Fatto**: il Re ha deciso: (1) la piattaforma v2 si chiama **SEAL**, repo canonico questo; (2) la resurrezione è un servizio — `seal-supervisor` (~100 righe, repo e deploy propri, API autonoma: chiamarla restarta seal); (3) bootstrap fresco + codice via prima sessione. Registrate come DEC-11…15 ratificate; nuova §7 (autogestione) con tabella codice/servizio/stato/resurrezione; §6.2 chiusa. Progetto `seal-supervisor` creato e spec scritta nel suo repo.
- **Scarti**: il naming — il Re ha unificato contratto/progetto/codice sotto SEAL, oltre la proposta «repo ivan-saorin/mgr». E l'auth del supervisor: niente OIDC, deve funzionare a stack mezzo morto.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica DEC-01…10; poi sequenza §5 passo 1.

### 2026-08-02 · nascita del repo + design MGR v2 v0.1 — driver: Kimi K3

- **Dove eravamo**: pattern nati nei progetti — torchio DEC-17/18 (recepimento, log operativo), tnt auth deployata (Authentik, gruppi, house key) — e lock di monosessione segnalato dal Re come scocciatura.
- **Previsione**: consolidare tutto in un documento di design della piattaforma.
- **Fatto**: progetto `seal` creato (repo `ivan-saorin/seal`); `docs/mgr-v2.md` v0.1 — auth a due gambe, log operativo a due layer, template versioning, concorrenza a riconciliazione con merge debt e compilazione di scrupolo; DEC-01…DEC-10; creato questo log (dogfood di DEC-04).
- **Scarti**: due, entrambi migliorativi e guidati da evidenza. (1) L'ipotesi iniziale «PAT mintati da console MGR» è crollata dopo la prova dal vivo su `auth.016180.xyz`: discovery pubblica attiva, token endpoint e device endpoint vivi — Authentik È già la console, i driver possono essere client OAuth2 di prima classe. (2) L'ipotesi del Re «device flow + refresh token nella skill» è stata analizzata e corretta nella forma: rotazione + concorrenza multi-chat la squalificano; la custodia passa lato MGR (DEC-02), mantenendo il device flow come gesto umano.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica delle DEC-01…10 da parte del Re; eventuale confronto incrociato con altro modello prima della ratifica.
