# SEAL — Log operativo

**Contratto.** Questo file è la memoria operativa del progetto. Ogni sessione che chiude lavoro coerente **aggiunge una voce in cima** (la più recente prima) e **aggiorna lo Stato attuale**. Ogni voce risponde a tre domande: **dove eravamo**, **dove prevedevamo di andare**, **cosa è stato fatto** — più gli **scarti** fra previsione e fatto, lo **stato a fine sessione** e il **prossimo passo previsto**. I commit message sono la memoria fine; questo log è la memoria grossa. Le voci non si cancellano: si correggono con voci nuove.

**Nota (MGR v2 DEC-04):** questo file è project-owned — nasce dal template alla creazione e il template non lo tocca mai più. Il layer macchina della storia (sessioni, merge, job) sarà esposto da MGR via `GET /mgr/projects/{p}/log`, senza scrivere nei repo.

---

## Stato attuale

_aggiornato: 2026-08-02, sessione ebb0b3c9_

- **Su master**: solo documentazione — `docs/mgr-v2.md` v0.2, questo log. Nessun codice.
- **Decisioni**: DEC-01…DEC-15 registrate. **DEC-11…15 (autogestione) ratificate dal Re 2026-08-02**; DEC-01…10 restano da ratificare.
- **Progetti di piattaforma**: `seal` (codice + contratto, questo repo), `seal-supervisor` (resurrezione; creato 2026-08-02, spec in `docs/requisiti.md` lì).
- **Prossimo passo previsto**: ratifica delle DEC-01…10; poi sequenza §5 passo 1 (`GET /mgr/contract` + tag vm-tmpl + pin in `mgr.toml`).

---

## Voci

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
