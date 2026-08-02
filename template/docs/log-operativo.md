# {project} — Log operativo

**Contratto.** Questo file è la memoria operativa del progetto. Ogni sessione che chiude lavoro coerente **aggiunge una voce in cima** (la più recente prima) e **aggiorna lo Stato attuale**. Ogni voce risponde a tre domande: **dove eravamo**, **dove prevedevamo di andare**, **cosa è stato fatto** — più gli **scarti** fra previsione e fatto, lo **stato a fine sessione** e il **prossimo passo previsto**. I commit message sono la memoria fine; questo log è la memoria grossa. Le voci non si cancellano: si correggono con voci nuove.

**Nota (DEC-04).** Questo file nasce dal template alla creazione del progetto e da quel momento è del progetto, per sempre: il template non lo tocca mai più, nessun sync lo riscrive. Il layer macchina della storia (sessioni, merge, job run) è esposto da SIGILED via `GET /mgr/projects/{project}/log`, senza scrivere nel repo.

---

## Stato attuale

_aggiornato: mai — progetto appena nato_

- Repo generato da vm-tmpl; nessuna sessione ha ancora chiuso lavoro.

---

## Voci

_(nessuna voce ancora — la prima sessione che chiude lavoro coerente scrive qui)_
