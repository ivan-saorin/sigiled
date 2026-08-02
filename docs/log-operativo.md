# SIGILED — Log operativo

**Contratto.** Questo file è la memoria operativa del progetto. Ogni sessione che chiude lavoro coerente **aggiunge una voce in cima** (la più recente prima) e **aggiorna lo Stato attuale**. Ogni voce risponde a tre domande: **dove eravamo**, **dove prevedevamo di andare**, **cosa è stato fatto** — più gli **scarti** fra previsione e fatto, lo **stato a fine sessione** e il **prossimo passo previsto**. I commit message sono la memoria fine; questo log è la memoria grossa. Le voci non si cancellano: si correggono con voci nuove.

**Nota (MGR v2 DEC-04):** questo file è project-owned — nasce dal template alla creazione e il template non lo tocca mai più. Il layer macchina della storia (sessioni, merge, job) sarà esposto da MGR via `GET /mgr/projects/{p}/log`, senza scrivere nei repo.

---

## Stato attuale

_aggiornato: 2026-08-03, sessione corrente (eradicazione «seal» → «sigiled»)_

- **Su master**: docs (`mgr-v2.md`, `v2-build-plan.md`, `sigiled-contract.md` 2.0.0-draft, questo log) + codice sessione 1: workspace cargo con `sigiledd/` (ex `seald/` — healthz, `GET /mgr/contract`, `GET /mgr/projects` con `template_version`; 7 unit test) e `vm-base/` (port del server v1, build-ext.sh su `ext-rust/`), `template/` (vm-tmpl v2), `images/build-vm-base.sh`, `tools/dev-toolchain.sh`, `CNAME` (`sigiled.dev`) + `index.html` (landing Pages).
- **RINOMINA COMPLETATA.** Per ordine diretto del Re («eradica completamente la stringa seal») il repo non contiene più alcuna occorrenza di «seal» in nessuna forma: codice (crate `sigiledd`, env `SIGILED_BUILD_SHA`, header `x-sigiled-version`), documenti, contratto, template, commenti, **voci storiche di questo log comprese**. La storia vera (la piattaforma nacque «MGR v2» → «SEAL» → SIGILED) è preservata dalla storia git, non più dal testo. Compilazione di scrupolo eseguita dopo lo sweep: `cargo build --workspace` verde (sigiledd + vm-base). Push del Re verificato: `ivan-saorin/sigiled` master = 2ab8f43, storia completa.
- **Decisioni**: DEC-01…DEC-20 registrate. DEC-11…20 ratificate; DEC-01…10 da ratificare — `sigiled-contract.md` resta draft finché non lo sono.
- **Progetti di piattaforma**: `sigiled` (codice + contratto — QUESTO repo, `ivan-saorin/sigiled`), il vecchio progetto MGR registrato col nome precedente (fondazione, da archiviare lato operatore su GitHub), `sigiled-supervisor` (resurrezione; nome confermato).
- **Per l'operatore**: (a) archiviare il vecchio repo su GitHub; (b) DNS `sigiled.dev` → 4×A GitHub Pages (185.199.108-111.153), `sigilled.dev` → 301 HTTPS; (c) abilitare Pages su `ivan-saorin/sigiled` (source: master, root); (d) build/push immagine `vm-base` su ghcr (`images/build-vm-base.sh`, DEC-20); (e) token API Authentik in stack env per la sessione 3.
- **Prossimo passo previsto**: progetto fresco `sigiled-supervisor` (requisiti già sweepati) + archiviazione del log del vecchio supervisor; poi **sessione 1b** (open source flip-ready: license, audit storia, ghcr pubblico, Pages live), poi sessione 2/4.

---

## Voci

### 2026-08-03 · eradicazione totale «seal» → «sigiled», build verde — driver: Kimi K3 (sessione corrente)

- **Dove eravamo**: push del Re fatto e verificato (master = 2ab8f43 su `ivan-saorin/sigiled`); sweep di rinomina già avviato in questa sessione (git mv `sigiled-contract.md`, `seald/` → `sigiledd/`, sed su Cargo e sorgenti Rust).
- **Fatto**: ordine diretto del Re — la stringa «seal» va eradicata ovunque, senza eccezioni. Sweep case-preserving su tutti i file tracciati (10 file residui oltre a quelli già trattati): documenti, contratto, commenti del codice, landing, **voci storiche di questo log**. Riparati i punti resi insensati dallo sweep (nota del contratto, titolo README, sostantivo «the sigil» nei commenti vm-base e nella regola 5). Compilazione di scrupolo: toolchain bootstrap + `cargo build --workspace` → **verde** (`sigiledd v2.0.0-alpha.1`, `vm-base`).
- **Scarti**: (1) la strategia iniziale del driver preservava il wordplay «seal/sealed» e i registri storici — ordine superiore ha prevalso, nessuna eccezione; (2) questo log perde consapevolmente la memoria testuale del nome precedente: la verità storica resta in git, il testo è uniforme.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: progetto `sigiled-supervisor` (genesi) + archiviazione log del vecchio supervisor; rename della skill locale; poi sessione 1b.

### 2026-08-03 · il nome: SIGILED — rinomina, domini, landing Pages — driver: Kimi K3 (sessione d2e77904)

- **Dove eravamo**: DEC-12 (nome SIGILED) ratificata ieri; DEC-19 (open source + landing gh-pages) ratificata in serata; restava il problema del dominio pubblico. Nel frattempo Claude Code aveva chiuso la sessione 1/4 (codice!) — il peso di questo repo è cambiato: non più solo docs.
- **Fatto**: esplorazione guidata dal Re con verifica RDAP sistematica (kunukku, editto, castellan, vsigiled, sigiled…); il Re ha scelto **sigiled** e acquistato la coppia `sigiled.dev` + `sigilled.dev` (il guardiano ortografico neutralizza l'ambiguità di spelling possedendo entrambe; `.ai` scartato: 83€/anno vs 13€/anno del `.dev`). Emendata **DEC-12**; sweep di rinomina su `mgr-v2.md` (titolo, header, §7, DEC-15); aggiunti `CNAME` (`sigiled.dev`) e `index.html` (landing pubblica) alla radice. Progetto MGR `sigiled` creato. Il contratto/skill resta **SIGILED** — piattaforma SIGILED, contratto SIGILED; questione aperta se uniformare.
- **Scarti**: (1) il nome scelto non è tra i quattro raccomandati dal driver (kunukku/editto/castellan/vsigiled) — il Re ha visto oltre; (2) la strategia di travaso è cambiata in corsa: invece di partire da repo fresco, TUTTO il contenuto (codice sessione 1 compreso) viaggia con la storia — il push è un comando solo dell'operatore, niente chirurgia sul DB di MGR.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: operatore pusha master di sigiled → `ivan-saorin/sigiled` (force, primo push); poi sessione su `sigiled` per la **sessione 1b** (flip-ready open source, DEC-19/20) — che include l'abilitazione della Pages su `sigiled.dev`; DNS: 4×A verso GitHub Pages su `sigiled.dev`, 301 HTTPS del guardiano `sigilled.dev`.

### 2026-08-02 · DEC-19/20 ratificate: open source al 100%, immagini pubbliche su ghcr; nasce la sessione 1b — driver: Claude Code (sessione b5245bba)

- **Dove eravamo**: sessione 1/4 chiusa in giornata; in chat, discussione su dove pubblicare le immagini base (prerequisito §1.3 del piano).
- **Fatto**: il Re ha ratificato in chat: (1) **DEC-19** — SIGILED v2 sarà open source al 100% con landing GitHub Pages; il repo si scrive da subito come pubblico; (2) **DEC-20** — immagini `vm-base:x.y.z` pubbliche su `ghcr.io/ivan-saorin` (pull senza credenziali, PAT solo per il push, nessun segreto nelle immagini per costruzione). Scritta la **sessione 1b** nel build plan (§2b): license (proposta Apache-2.0), audit igiene della storia git, verifica naming (SIGILED collide con altri progetti), generalizzazione degli stack-specifics in config, FROM/script su ghcr, README EN + landing.
- **Scarti**: la decisione sul registry, lasciata all'operatore dalla sessione 1, si è risolta in chat ed è diventata più grande: non «quale registry» ma «il progetto è pubblico». Tentato l'accesso SSH al box dal PC dell'operatore per la build di vm-base: chiave non autorizzata (e host .205 con host key cambiata — segnalato); la build resta all'operatore con le istruzioni lasciate in chat.
- **Stato a fine sessione**: vedi «Stato attuale».
- **Prossimo passo previsto**: sessione 1b, poi sessione 2/4.

### 2026-08-02 · sessione 1/4 chiusa: fondamenta v2 su master — driver: Claude Code (sessione 03fe1b7d)

- **Dove eravamo**: solo docs su master; assessment appena registrato (voce sotto): greenfield per sigiledd, evoluzione per vm-base.
- **Previsione**: i deliverable di §2 del piano — layout, healthz+contract, parse del pin template, template v2, script immagine.
- **Fatto**: tutto §2. (1) Layout: workspace cargo, `server/`→`vm-base/` via git mv, `build-ext.sh` su convenzione `ext-rust/`, Dockerfile root a doppio ruolo (compat sessioni MGR v1 + sorgente di `vm-base:x.y.z`). (2) `docs/sigiled-contract.md` 2.0.0-draft generato da mgr-v2.md, embedded in sigiledd a compile time (`GET /mgr/contract` serve allo sha del build per costruzione). (3) sigiledd: `/healthz`, `/mgr/contract`, `/mgr/projects` con `template_version` dal pin `template = "vm-tmpl@x.y.z"` (parser con 7 unit test, incluso il test che il manifest del template non regredisca). (4) `template/`: Dockerfile sottile FROM vm-base:0.1.0, mgr.toml commentato col pin, skeleton log-operativo, ext-rust/ vuota. (5) `images/build-vm-base.sh` (versione da vm-base/Cargo.toml). Acceptance verificata: build+test verdi, run locale su :8099 risponde su tutti e tre gli endpoint.
- **Scarti**: uno solo, strumentale — il container di sessione (runtime v1, debian-slim) non ha né rust né cc: per «build e test dentro la sessione» è nato `tools/dev-toolchain.sh` (rustup + gcc-14/binutils/libc6-dev estratti da deb.debian.org in `$HOME/tc` senza root, ~2 min; effimero, da rilanciare dopo recycle). Da valutare in sessione 2 se l'immagine v2 debba includere la toolchain di serie.
- **Stato a fine sessione**: vedi «Stato attuale» sopra. Master chiude verde.
- **Prossimo passo previsto**: sessione 2/4 (log macchina + recepimento); per l'operatore, build+push di vm-base:0.1.0 e registry (§1.3), token Authentik in stack env per la sessione 3 (§1.2).

### 2026-08-02 · sessione 1/4, assessment iniziale: greenfield per sigiledd, evoluzione per vm-base — driver: Claude Code (sessione 03fe1b7d)

- **Dove eravamo**: piano in 4 sessioni su master; il prerequisito §1.1 (push del codice MGR v1 o decisione esplicita di greenfield) non era stato consumato.
- **Assessment** (il bivio del piano, risolto sull'evidenza del repo):
  1. Il codice dell'orchestratore MGR v1 **non è su questo repo**: su master c'è solo il vendoring vm-tmpl v1 (`server/`, `ext/`, `build-ext.sh`, `Dockerfile`, `mgr.toml`) dell'Initial commit.
  2. `sigiledd/` nasce **greenfield guidato**: la spec del comportamento v1 è il contratto SIGILED (la skill), la v2 è `docs/mgr-v2.md` — riscrivere in Rust da spec (DEC-16) è più pulito che re-importare un codice che qui non esiste.
  3. `vm-base/` è **evoluzione**: il server axum vendorizzato è già Rust e già in produzione come agent dei workspace — si porta con `git mv` e diventa la sorgente dell'immagine `vm-base:x.y.z` (DEC-17).
  4. `template/` si riscrive sottile per costruzione (DEC-17/18): `FROM vm-base` + skeleton docs + `mgr.toml` commentato + `ext-rust/` d'esempio.
  5. Vincolo pratico: il `Dockerfile` di root deve restare funzionante — MGR **v1** lo usa per costruire l'immagine di sessione di questo stesso repo — quindi si adatta a puntare a `vm-base/`, non si elimina.
- **Prossimo passo previsto**: layout repo, poi sigiledd (healthz + contract + parse template pin), template v2, script immagine. Voce di chiusura a fine sessione.

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

### 2026-08-02 · v0.1 → v0.2: autogestione ratificata, nasce sigiled-supervisor — driver: Kimi K3

- **Dove eravamo**: design v0.1 su master; questione aperta §6.2 — MGR MGR-registrato?
- **Previsione**: discussione in chat su implicazioni e limiti dell'autogestione.
- **Fatto**: il Re ha deciso: (1) la piattaforma v2 si chiama **SIGILED**, repo canonico questo; (2) la resurrezione è un servizio — `sigiled-supervisor` (~100 righe, repo e deploy propri, API autonoma: chiamarla restarta sigiled); (3) bootstrap fresco + codice via prima sessione. Registrate come DEC-11…15 ratificate; nuova §7 (autogestione) con tabella codice/servizio/stato/resurrezione; §6.2 chiusa. Progetto `sigiled-supervisor` creato e spec scritta nel suo repo.
- **Scarti**: il naming — il Re ha unificato contratto/progetto/codice sotto SIGILED, oltre la proposta «repo ivan-saorin/mgr». E l'auth del supervisor: niente OIDC, deve funzionare a stack mezzo morto.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica DEC-01…10; poi sequenza §5 passo 1.

### 2026-08-02 · nascita del repo + design MGR v2 v0.1 — driver: Kimi K3

- **Dove eravamo**: pattern nati nei progetti — torchio DEC-17/18 (recepimento, log operativo), tnt auth deployata (Authentik, gruppi, house key) — e lock di monosessione segnalato dal Re come scocciatura.
- **Previsione**: consolidare tutto in un documento di design della piattaforma.
- **Fatto**: progetto `sigiled` creato (repo `ivan-saorin/sigiled`); `docs/mgr-v2.md` v0.1 — auth a due gambe, log operativo a due layer, template versioning, concorrenza a riconciliazione con merge debt e compilazione di scrupolo; DEC-01…DEC-10; creato questo log (dogfood di DEC-04).
- **Scarti**: due, entrambi migliorativi e guidati da evidenza. (1) L'ipotesi iniziale «PAT mintati da console MGR» è crollata dopo la prova dal vivo su `auth.016180.xyz`: discovery pubblica attiva, token endpoint e device endpoint vivi — Authentik È già la console, i driver possono essere client OAuth2 di prima classe. (2) L'ipotesi del Re «device flow + refresh token nella skill» è stata analizzata e corretta nella forma: rotazione + concorrenza multi-chat la squalificano; la custodia passa lato MGR (DEC-02), mantenendo il device flow come gesto umano.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica delle DEC-01…10 da parte del Re; eventuale confronto incrociato con altro modello prima della ratifica.
