//! Adattatori reali per la conduzione: un dominio `cgroup2` vero, non un
//! test.
//!
//! Implementano i tre contratti che [`super::produttori::Osservatore`],
//! [`super::conduzione::Terminatore`] e [`super::conduzione::LettoreDiEvidenza`]
//! dichiarano, leggendo e scrivendo i file veri del dominio. Il giudizio —
//! quiescente o no, quale evidenza autorizza l'attribuzione — resta dove e'
//! gia' costruito e provato: [`crate::classificazione::classifica`]. Qui si
//! legge, non si giudica: ogni lettura mancata o incoerente diventa `None` o
//! [`Difetto`], mai un valore inventato.
//!
//! # Fino a dove si guardano gli antenati
//!
//! `Oa` (`PressioneDegliAntenati`) cammina dal genitore del dominio fino
//! alla **radice del control plane** compresa, e non oltre. E' lo stesso
//! confine che [`super::super::accerta_perimetro`] gia' usa per giudicare il
//! possesso: e' l'unico confine garantito raggiungibile qualunque sia il
//! dispiegamento, perche' e' quello che `PLENORA_ISOLATION_CGROUP_ROOT`
//! dichiara. Andare oltre significherebbe leggere cgroup che questo
//! dispiegamento non ha dichiarato di governare — path che potrebbero non
//! esistere affatto se il supervisore vede solo il sottoalbero delegato.

use std::path::{Path, PathBuf};

use plenora_core::error::{
    DiagnosticaSupplementare, EvidenzaDiLimite, PressioneDegliAntenati, MAX_ANTENATI_OSSERVATI,
};

use super::conduzione::{LettoreDiEvidenza, Terminatore};
use super::produttori::{Difetto, Osservatore};

/// Un contatore da `memory.events`/`memory.events.local`: `chiave valore` per
/// riga, come `cgroup.events`.
///
/// # Perche' sempre `Option`, mai un errore separato
///
/// Perche' [`EvidenzaDiLimite`] non distingue «il file manca», «la chiave
/// manca» e «la chiave compare due volte»: tutte e tre dicono la stessa cosa
/// a chi giudica, cioe' che quel numero non e' un'osservazione su cui
/// contare. Distinguerle qui per poi appiattirle subito dopo aggiungerebbe
/// una forma senza un lettore.
fn contatore(testo: &str, chiave: &str) -> Option<u64> {
    let mut trovato = None;
    for riga in testo.lines() {
        let (nome, valore) = riga.split_once(' ')?;
        if nome.trim() != chiave {
            continue;
        }
        if trovato.is_some() {
            // La chiave compare due volte: quale valga non lo dice nessuno,
            // esattamente come `popolato` rifiuta lo stesso caso su
            // `cgroup.events`.
            return None;
        }
        trovato = valore.trim().parse().ok();
    }
    trovato
}

fn leggi_contatore(percorso: &Path, chiave: &str) -> Option<u64> {
    super::super::lettura::leggi_limitato(percorso)
        .ok()
        .and_then(|testo| contatore(&testo, chiave))
}

/// Guarda se il dominio si e' svuotato, leggendo `cgroup.events` per davvero.
pub(super) struct SorvegliaDominio {
    dominio: PathBuf,
}

impl SorvegliaDominio {
    pub(super) const fn nuova(dominio: PathBuf) -> Self {
        Self { dominio }
    }
}

/// Traduce il difetto di una lettura reale nel `Difetto` che i produttori
/// dichiarano.
///
/// # Perche' `Interrotta` e non sempre `Impossibile`
///
/// Perche' un `ErrorKind::Interrupted` non e' una lettura mancata: e' un
/// segnale arrivato nel mezzo, e la lettura successiva la fa avvenire senza
/// che l'evidenza ne resti incompleta. Trattarla come `Impossibile`
/// riporterebbe un difetto nostro per un evento che il giro dopo risolve da
/// solo — esattamente cio' che [`Difetto::Interrotta`] esiste per evitare.
fn difetto_da_lettura(difetto: super::super::DifettoSuperficie) -> Difetto {
    match difetto {
        super::super::DifettoSuperficie::Lettura { causa, .. }
        | super::super::DifettoSuperficie::Scrittura { causa, .. }
            if causa.kind() == std::io::ErrorKind::Interrupted =>
        {
            Difetto::Interrotta
        }
        altro => Difetto::Impossibile(altro.to_string()),
    }
}

impl Osservatore for SorvegliaDominio {
    fn quiescente(&mut self) -> std::result::Result<bool, Difetto> {
        let eventi = super::super::lettura::leggi_limitato(&self.dominio.join("cgroup.events"))
            .map_err(difetto_da_lettura)?;
        super::super::popolato(&eventi)
            .map(|occupato| !occupato)
            .map_err(|motivo| Difetto::Impossibile(motivo.to_owned()))
    }
}

/// Sa svuotare il dominio scrivendo `cgroup.kill`.
///
/// # Perche' non passa da `SuperficieDominio::scrivi`
///
/// Perche' quel metodo scrive **solo** i quattro controlli del preflight
/// (`Controllo::ORDINE`), con la semantica «il file esiste gia', non lo
/// creo»: `cgroup.kill` non e' uno di quei quattro, e non ha bisogno della
/// rilettura verificata che quel percorso impone — scrivere e' l'unica
/// operazione che il kernel gli riconosce.
pub(super) struct TerminaDominio {
    dominio: PathBuf,
}

impl TerminaDominio {
    pub(super) const fn nuova(dominio: PathBuf) -> Self {
        Self { dominio }
    }
}

impl Terminatore for TerminaDominio {
    fn termina(&mut self) -> std::result::Result<(), String> {
        std::fs::write(self.dominio.join("cgroup.kill"), "1")
            .map_err(|causa| format!("cgroup.kill: {causa}"))
    }
}

/// I contatori che contano per l'evidenza, letti in un solo istante.
///
/// Presa due volte — prima dello spawn e dopo la quiescenza — cosi' che
/// [`LeggiEvidenzaDominio::evidenza`] possa rendere un **delta**
/// nell'intervallo del tentativo, non un valore assoluto che confonderebbe
/// la pressione di un tentativo precedente con quella di questo.
#[derive(Debug, Default)]
struct Istantanea {
    oom_locale: Option<u64>,
    uccisi_locale: Option<u64>,
    oom_group_kill_locale: Option<u64>,
    uccisi_gerarchia: Option<u64>,
    picco_byte: Option<u64>,
    respinte_al_tetto: Option<u64>,
    /// Un contatore per antenato, nello stesso ordine di
    /// [`antenati_fino_alla_radice`]: `[0]` il genitore.
    antenati_oom_locale: Vec<Option<u64>>,
}

fn istantanea(dominio: &Path, antenati: &[PathBuf]) -> Istantanea {
    let locali = dominio.join("memory.events.local");
    let gerarchici = dominio.join("memory.events");
    Istantanea {
        oom_locale: leggi_contatore(&locali, "oom"),
        uccisi_locale: leggi_contatore(&locali, "oom_kill"),
        oom_group_kill_locale: leggi_contatore(&locali, "oom_group_kill"),
        uccisi_gerarchia: leggi_contatore(&gerarchici, "oom_kill"),
        picco_byte: leggi_intero_semplice(&dominio.join("memory.peak")),
        respinte_al_tetto: leggi_contatore(&locali, "max"),
        antenati_oom_locale: antenati
            .iter()
            .map(|antenato| leggi_contatore(&antenato.join("memory.events.local"), "oom"))
            .collect(),
    }
}

/// `memory.peak` non e' `chiave valore`: e' un intero solo, su una riga sola.
fn leggi_intero_semplice(percorso: &Path) -> Option<u64> {
    super::super::lettura::leggi_limitato(percorso)
        .ok()
        .and_then(|testo| testo.trim().parse().ok())
}

/// Il delta fra due letture dello stesso contatore, o `None` se una delle
/// due manca o se il delta sarebbe negativo.
///
/// # Perche' un delta negativo e' `None` e non zero
///
/// Perche' un contatore di `cgroup2` e' monotono per tutta la vita del
/// dominio: se la lettura «dopo» e' minore della lettura «prima», qualcosa
/// nella lettura stessa non regge — non e' una diminuzione vera. Riportarla
/// come zero la farebbe sembrare «nessuna pressione», che e' esattamente il
/// contrario di un'evidenza che non torna.
fn delta(prima: Option<u64>, dopo: Option<u64>) -> Option<u64> {
    dopo.and_then(|d| prima.and_then(|p| d.checked_sub(p)))
}

/// I percorsi dal genitore del dominio fino alla radice del control plane
/// compresa, capati a [`MAX_ANTENATI_OSSERVATI`].
///
/// Se la radice non si raggiunge entro il tetto, gli antenati oltre
/// restano non contati: [`LeggiEvidenzaDominio::evidenza`] lo dichiara in
/// [`PressioneDegliAntenati::nuova`] con `antenati_oltre_capacita`, mai in
/// silenzio.
fn antenati_fino_alla_radice(dominio: &Path, radice: &Path) -> (Vec<PathBuf>, u32) {
    let mut antenati = Vec::new();
    let mut corrente = dominio;
    let mut oltre = 0_u32;
    while let Some(genitore) = corrente.parent() {
        if antenati.len() >= MAX_ANTENATI_OSSERVATI {
            oltre = oltre.saturating_add(1);
            if genitore == radice {
                break;
            }
            corrente = genitore;
            continue;
        }
        antenati.push(genitore.to_path_buf());
        if genitore == radice {
            break;
        }
        corrente = genitore;
    }
    (antenati, oltre)
}

/// Legge l'evidenza del dominio dopo la quiescenza, come delta rispetto a
/// un'istantanea presa prima dello spawn.
pub(super) struct LeggiEvidenzaDominio {
    dominio: PathBuf,
    antenati: Vec<PathBuf>,
    antenati_oltre_capacita: u32,
    tetto_byte: u64,
    prima: Istantanea,
}

impl LeggiEvidenzaDominio {
    /// Prende l'istantanea **subito**, prima che il worker parta: e' il
    /// solo momento in cui «prima» significa davvero prima.
    pub(super) fn nuova(dominio: PathBuf, radice: &Path, tetto_byte: u64) -> Self {
        let (antenati, antenati_oltre_capacita) = antenati_fino_alla_radice(&dominio, radice);
        let prima = istantanea(&dominio, &antenati);
        Self {
            dominio,
            antenati,
            antenati_oltre_capacita,
            tetto_byte,
            prima,
        }
    }
}

impl LettoreDiEvidenza for LeggiEvidenzaDominio {
    fn evidenza(&mut self) -> std::result::Result<EvidenzaDiLimite, Difetto> {
        let dopo = istantanea(&self.dominio, &self.antenati);

        let letture_antenati: Vec<Option<u64>> = self
            .prima
            .antenati_oom_locale
            .iter()
            .zip(dopo.antenati_oom_locale.iter())
            .map(|(&p, &d)| delta(p, d))
            .collect();
        let oom_degli_antenati = PressioneDegliAntenati::nuova(
            &letture_antenati,
            Some(self.antenati.len()),
            self.antenati_oltre_capacita,
        )
        .map_err(|forma| Difetto::Impossibile(forma.to_string()))?;

        Ok(EvidenzaDiLimite {
            oom_locali: delta(self.prima.oom_locale, dopo.oom_locale),
            uccisi_nel_dominio: delta(self.prima.uccisi_locale, dopo.uccisi_locale),
            uccisi_nella_gerarchia: delta(self.prima.uccisi_gerarchia, dopo.uccisi_gerarchia),
            group_kill_locale: delta(self.prima.oom_group_kill_locale, dopo.oom_group_kill_locale),
            oom_degli_antenati,
            diagnostica: DiagnosticaSupplementare {
                tetto_byte: self.tetto_byte,
                picco_byte: dopo.picco_byte,
                respinte_al_tetto: delta(self.prima.respinte_al_tetto, dopo.respinte_al_tetto),
            },
        })
    }
}
