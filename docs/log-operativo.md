# SEAL — Log operativo

**Contratto.** Questo file è la memoria operativa del progetto. Ogni sessione che chiude lavoro coerente **aggiunge una voce in cima** (la più recente prima) e **aggiorna lo Stato attuale**. Ogni voce risponde a tre domande: **dove eravamo**, **dove prevedevamo di andare**, **cosa è stato fatto** — più gli **scarti** fra previsione e fatto, lo **stato a fine sessione** e il **prossimo passo previsto**. I commit message sono la memoria fine; questo log è la memoria grossa. Le voci non si cancellano: si correggono con voci nuove.

**Nota (MGR v2 DEC-04):** questo file è project-owned — nasce dal template alla creazione e il template non lo tocca mai più. Il layer macchina della storia (sessioni, merge, job) sarà esposto da MGR via `GET /mgr/projects/{p}/log`, senza scrivere nei repo.

---

## Stato attuale

_aggiornato: 2026-08-02, sessione 4f458f6f_

- **Su master**: solo documentazione — `docs/mgr-v2.md` v0.1, questo log. Nessun codice.
- **Decisioni**: DEC-01…DEC-10 registrate, tutte **da ratificare** dal Re.
- **Prossimo passo previsto**: ratifica delle DEC; poi sequenza §5 passo 1 (`GET /mgr/contract` + tag vm-tmpl + pin in `mgr.toml`). Questione aperta prioritaria: il repo di MGR stesso è MGR-registrato? (§6.2)

---

## Voci

### 2026-08-02 · nascita del repo + design MGR v2 v0.1 — driver: Kimi K3

- **Dove eravamo**: pattern nati nei progetti — torchio DEC-17/18 (recepimento, log operativo), tnt auth deployata (Authentik, gruppi, house key) — e lock di monosessione segnalato dal Re come scocciatura.
- **Previsione**: consolidare tutto in un documento di design della piattaforma.
- **Fatto**: progetto `seal` creato (repo `ivan-saorin/seal`); `docs/mgr-v2.md` v0.1 — auth a due gambe, log operativo a due layer, template versioning, concorrenza a riconciliazione con merge debt e compilazione di scrupolo; DEC-01…DEC-10; creato questo log (dogfood di DEC-04).
- **Scarti**: due, entrambi migliorativi e guidati da evidenza. (1) L'ipotesi iniziale «PAT mintati da console MGR» è crollata dopo la prova dal vivo su `auth.016180.xyz`: discovery pubblica attiva, token endpoint e device endpoint vivi — Authentik È già la console, i driver possono essere client OAuth2 di prima classe. (2) L'ipotesi del Re «device flow + refresh token nella skill» è stata analizzata e corretta nella forma: rotazione + concorrenza multi-chat la squalificano; la custodia passa lato MGR (DEC-02), mantenendo il device flow come gesto umano.
- **Stato a fine sessione**: vedi «Stato attuale» sopra.
- **Prossimo passo previsto**: ratifica delle DEC-01…10 da parte del Re; eventuale confronto incrociato con altro modello prima della ratifica.
