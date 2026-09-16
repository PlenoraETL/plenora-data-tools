//! La superficie vera: una directory di `cgroup2`.
//!
//! Non decide niente. Risolve, apre, scrive, legge, e riporta il difetto
//! meccanico a chi lo deve interpretare — che e' il preflight, e sta altrove
//! apposta.

use std::io::Write as _;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use super::lettura::leggi_limitato;
use super::{Controllo, DifettoSuperficie, Montaggio, ProprietaFile, SuperficieDominio};

/// Dove si legge come sono montati i filesystem di questo processo.
///
/// `mountinfo` e non `mounts`: solo il primo porta l'identita' del filesystem
/// (`major:minor`) e la **radice** del mount, e senza quei due campi non si
/// puo' ne' riconoscere lo stesso filesystem raggiunto per un altro percorso,
/// ne' calcolare un percorso corretto attraverso un bind mount di sottoalbero.
const MOUNTINFO: &str = "/proc/self/mountinfo";

/// Il tipo che si cerca in quel file.
const TIPO_CGROUP2: &str = "cgroup2";

type Esito<T> = std::result::Result<T, DifettoSuperficie>;

/// Una directory della gerarchia `cgroup2`, con la radice del control plane
/// fino a cui il possesso va giudicato.
///
/// # I percorsi si risolvono **una volta**, alla costruzione
///
/// Non a ogni chiamata, e soprattutto non solo dove servono per il confronto:
/// se `dominio()` rendesse il canonico e le scritture usassero il percorso
/// nominato, il preflight giudicherebbe un percorso e ne modificherebbe un
/// altro ogni volta che nel mezzo c'e' un link simbolico o un `..`. Qui il
/// canonico e' l'unico che esiste dopo la costruzione.
pub(super) struct Gerarchia {
    dominio: PathBuf,
    radice: PathBuf,
}

impl Gerarchia {
    /// # Errors
    ///
    /// [`DifettoSuperficie::Lettura`] se uno dei due percorsi non si risolve:
    /// un dominio che non esiste non e' un dominio da preparare.
    pub(super) fn nuova(dominio: &Path, radice: &Path) -> Esito<Self> {
        Ok(Self {
            dominio: canonico(dominio)?,
            radice: canonico(radice)?,
        })
    }
}

/// Risolve un percorso, e riporta il fallimento con il percorso dentro.
fn canonico(percorso: &Path) -> Esito<PathBuf> {
    percorso
        .canonicalize()
        .map_err(|errore| DifettoSuperficie::lettura(percorso.display().to_string(), errore))
}

impl SuperficieDominio for Gerarchia {
    fn dominio(&self) -> Esito<PathBuf> {
        Ok(self.dominio.clone())
    }

    fn radice_control_plane(&self) -> Esito<PathBuf> {
        Ok(self.radice.clone())
    }

    fn montaggio(&self, dominio: &Path) -> Esito<Montaggio> {
        let mountinfo = leggi_limitato(Path::new(MOUNTINFO))?;
        montaggio_che_contiene(&mountinfo, dominio)
    }

    fn proprieta(&self, percorso: &Path) -> Esito<ProprietaFile> {
        // `symlink_metadata` e non `metadata`: seguire un link direbbe di un
        // altro file, e nella gerarchia non ce ne sono — quindi un link qui e'
        // gia' un ambiente diverso da quello atteso, e va giudicato per quello
        // che e' invece che per cio' a cui punta.
        let dati = std::fs::symlink_metadata(percorso)
            .map_err(|errore| DifettoSuperficie::lettura(percorso.display().to_string(), errore))?;
        Ok(ProprietaFile {
            uid: dati.uid(),
            gid: dati.gid(),
            mode: dati.mode(),
        })
    }

    #[cfg(any(test, feature = "internals"))]
    fn namespace(&self) -> Esito<Vec<(String, String)>> {
        super::identita::namespace_di_self().map_err(DifettoSuperficie::Forma)
    }

    #[cfg(any(test, feature = "internals"))]
    fn scrivi(&mut self, controllo: Controllo, valore: &str) -> Esito<()> {
        let percorso = self.dominio.join(controllo.file());
        let nome = percorso.display().to_string();
        // `write` e non `create`: i file di un cgroup esistono gia', e crearne
        // uno significa aver sbagliato directory. Un `OpenOptions` che non crea
        // trasforma quell'errore in un fallimento invece che in un file
        // inutile.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&percorso)
            .map_err(|causa| DifettoSuperficie::scrittura(nome.clone(), causa))?;
        file.write_all(valore.as_bytes())
            .map_err(|causa| DifettoSuperficie::scrittura(nome, causa))
    }

    fn rileggi(&self, controllo: Controllo) -> Esito<String> {
        leggi_limitato(&self.dominio.join(controllo.file()))
    }

    fn eventi(&self) -> Esito<String> {
        leggi_limitato(&self.dominio.join("cgroup.events"))
    }
}

/// Il montaggio `cgroup2` che contiene quel percorso.
///
/// # Perche' «che contiene» e non «il primo»
///
/// Con piu' di un `cgroup2` montato — o con un bind mount di un sottoalbero —
/// prendere il primo significa registrare le opzioni di un filesystem e
/// calcolare l'appartenenza su un altro. Sono due affermazioni su due oggetti
/// diversi, e nulla nel codice direbbe che non parlano della stessa cosa.
///
/// Si sceglie quello il cui punto di mount e' il prefisso **piu' lungo** del
/// percorso: e' il mount attraverso cui quel percorso e' effettivamente
/// raggiunto, perche' un mount piu' profondo copre quello che sta sopra. Il
/// confronto e' fra `Path`, quindi per **componenti**: `/sys/fs/cgroup2` non e'
/// un prefisso di `/sys/fs/cgroup/x` per quanto lo sia il suo testo.
///
/// # Perche' ogni riga deve interpretarsi
///
/// Una riga che non si legge non si salta. `/proc/self/mountinfo` lo scrive il
/// kernel in un formato fisso: una riga che non lo rispetta significa che il
/// formato e' cambiato sotto di noi, e continuare vorrebbe dire scegliere un
/// montaggio avendo ignorato proprio la riga che non si capisce — che
/// potrebbe essere quella giusta.
///
/// # Errors
///
/// [`DifettoSuperficie::Lettura`] se una riga non si interpreta, se nessun
/// `cgroup2` contiene il percorso, o se piu' d'uno lo contiene ugualmente bene:
/// l'ambiguita' qui non ha una risposta di ripiego.
fn montaggio_che_contiene(mountinfo: &str, percorso: &Path) -> Esito<Montaggio> {
    let mut migliori: Vec<Montaggio> = Vec::new();
    let mut lunghezza_migliore = 0_usize;

    for (numero, riga) in mountinfo.lines().enumerate() {
        if riga.trim().is_empty() {
            continue;
        }
        let voce = voce_di_mountinfo(riga).map_err(|motivo| {
            DifettoSuperficie::Forma(format!("{MOUNTINFO}, riga {}: {motivo}", numero + 1))
        })?;
        if voce.tipo != TIPO_CGROUP2 || !percorso.starts_with(&voce.punto) {
            continue;
        }
        let lunghezza = voce.punto.components().count();
        let montaggio = Montaggio {
            punto: voce.punto,
            radice: voce.radice,
            opzioni_mount: voce.opzioni_mount,
            opzioni_superblocco: voce.opzioni_superblocco,
            dispositivo: voce.dispositivo,
        };
        if lunghezza > lunghezza_migliore {
            lunghezza_migliore = lunghezza;
            migliori.clear();
            migliori.push(montaggio);
        } else if lunghezza == lunghezza_migliore {
            migliori.push(montaggio);
        }
    }

    match migliori.len() {
        1 => Ok(migliori.remove(0)),
        0 => Err(DifettoSuperficie::Forma(format!(
            "nessun cgroup2 contiene {}",
            percorso.display()
        ))),
        quanti => Err(DifettoSuperficie::Forma(format!(
            "{quanti} montaggi cgroup2 contengono {} allo stesso livello: quale sia quello \
             giusto non lo dice l'ordine delle righe",
            percorso.display()
        ))),
    }
}

/// I campi che servono di una riga di `mountinfo`.
struct Voce {
    punto: PathBuf,
    radice: PathBuf,
    opzioni_mount: String,
    opzioni_superblocco: String,
    dispositivo: String,
    tipo: String,
}

/// Una riga di `/proc/self/mountinfo`.
///
/// Il formato ha campi posizionali fino a un numero **variabile** di campi
/// opzionali, chiusi da un `-` isolato; dopo il trattino vengono tipo, sorgente
/// e opzioni del superblocco. Contare le posizioni oltre il trattino senza
/// cercarlo darebbe campi sbagliati esattamente sulle macchine che hanno campi
/// opzionali, cioe' quelle con propagazione fra mount.
///
/// # Errors
///
/// Il motivo: un campo che manca o un escape che non e' fra quelli che il
/// kernel emette.
fn voce_di_mountinfo(riga: &str) -> std::result::Result<Voce, String> {
    let mut campi = riga.split(' ');
    let mut prossimo = |quale: &str| -> std::result::Result<&str, String> {
        campi
            .next()
            .ok_or_else(|| format!("manca il campo {quale}"))
    };
    prossimo("id")?;
    prossimo("id del padre")?;
    let dispositivo = prossimo("dispositivo")?.to_owned();
    let radice = decodifica(prossimo("radice")?)?;
    let punto = decodifica(prossimo("punto di mount")?)?;
    let opzioni_mount = prossimo("opzioni del mount")?.to_owned();
    // I campi opzionali, fino al trattino isolato.
    loop {
        match campi.next() {
            Some("-") => break,
            Some(_) => {}
            None => return Err("manca il trattino che chiude i campi opzionali".to_owned()),
        }
    }
    let tipo = campi
        .next()
        .ok_or_else(|| "manca il tipo di filesystem".to_owned())?
        .to_owned();
    campi
        .next()
        .ok_or_else(|| "manca la sorgente del mount".to_owned())?;
    let opzioni_superblocco = campi
        .next()
        .ok_or_else(|| "mancano le opzioni del superblocco".to_owned())?
        .to_owned();
    // Le opzioni del superblocco sono l'ultimo campo del formato. Un campo in
    // piu' significa che il formato non e' quello che crediamo, e continuare
    // vorrebbe dire aver letto i campi precedenti per posizione in una riga
    // che quelle posizioni non le rispetta.
    if let Some(altro) = campi.next() {
        return Err(format!(
            "campo inatteso dopo le opzioni del superblocco: «{altro}»"
        ));
    }
    // `major:minor`, due interi separati da due punti. E' l'identita' con cui
    // si riconosce lo stesso filesystem raggiunto per un altro percorso:
    // accettarne una forma qualunque significherebbe confrontare piu' tardi
    // due stringhe di cui non sappiamo niente.
    let (maggiore, minore) = dispositivo
        .split_once(':')
        .ok_or_else(|| format!("il dispositivo «{dispositivo}» non ha forma major:minor"))?;
    for (nome, cifre) in [("major", maggiore), ("minor", minore)] {
        if cifre.is_empty() || !cifre.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "il {nome} del dispositivo «{dispositivo}» non e' un intero"
            ));
        }
    }
    Ok(Voce {
        punto,
        radice,
        opzioni_mount,
        opzioni_superblocco,
        dispositivo,
        tipo,
    })
}

/// Decodifica gli escape ottali dei percorsi di `mountinfo`.
///
/// # Perche' sui byte e non sui caratteri
///
/// Un percorso su Linux e' una sequenza di **byte**, non di caratteri, e non e'
/// tenuto a essere UTF-8. Decodificare carattere per carattere spezzerebbe ogni
/// sequenza multibyte in byte separati e la ricomporrebbe sbagliata: un
/// percorso con un accento diventerebbe un percorso diverso, e il confronto per
/// prefisso fallirebbe senza che nulla lo segnali.
///
/// Qui si lavora sui byte e si costruisce un `OsString` da quei byte: cio' che
/// entra esce identico, UTF-8 o no.
///
/// # Perche' un escape ignoto e' un errore
///
/// Il kernel ne emette **quattro**: spazio, tab, newline, backslash. Accettarne
/// altri significherebbe interpretare una sequenza che nessuno ha dichiarato,
/// e un percorso interpretato male non e' quel percorso.
///
/// # Errors
///
/// Il motivo, se un escape non e' fra i quattro o e' troncato.
fn decodifica(campo: &str) -> std::result::Result<PathBuf, String> {
    /// Le quattro sequenze che il kernel emette, e nessun'altra.
    const AMMESSI: [(&str, u8); 4] = [
        ("040", b' '),
        ("011", b'\t'),
        ("012", b'\n'),
        ("134", b'\\'),
    ];

    let byte = campo.as_bytes();
    let mut fuori: Vec<u8> = Vec::with_capacity(byte.len());
    let mut indice = 0_usize;
    while indice < byte.len() {
        if byte[indice] != b'\\' {
            fuori.push(byte[indice]);
            indice += 1;
            continue;
        }
        let cifre = byte
            .get(indice + 1..indice + 4)
            .and_then(|tre| std::str::from_utf8(tre).ok())
            .ok_or_else(|| format!("escape troncato in «{campo}»"))?;
        let valore = AMMESSI
            .iter()
            .find_map(|(sequenza, valore)| (*sequenza == cifre).then_some(*valore))
            .ok_or_else(|| {
                format!("escape \\{cifre} non e' fra quelli che il kernel emette, in «{campo}»")
            })?;
        fuori.push(valore);
        indice += 4;
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(fuori)))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{decodifica, montaggio_che_contiene};

    const DUE_MOUNT: &str = "\
23 28 0:22 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
29 23 0:27 / /sys/fs/cgroup rw,nosuid,nodev,noexec,relatime shared:9 - cgroup2 cgroup2 rw,nsdelegate
44 29 0:27 /plenora /mnt/dominio rw,nosuid,relatime shared:9 - cgroup2 cgroup2 rw,memory_localevents
";

    /// Si sceglie il montaggio che **contiene** il dominio, non il primo.
    #[test]
    fn si_sceglie_il_montaggio_che_contiene_il_dominio() {
        let sotto_il_secondo =
            montaggio_che_contiene(DUE_MOUNT, Path::new("/mnt/dominio/lavoro")).expect("montaggio");
        assert_eq!(sotto_il_secondo.punto, PathBuf::from("/mnt/dominio"));
        assert_eq!(sotto_il_secondo.radice, PathBuf::from("/plenora"));
        assert!(sotto_il_secondo
            .opzioni_superblocco
            .contains("memory_localevents"));

        let sotto_il_primo = montaggio_che_contiene(DUE_MOUNT, Path::new("/sys/fs/cgroup/altro"))
            .expect("montaggio");
        assert_eq!(sotto_il_primo.punto, PathBuf::from("/sys/fs/cgroup"));
        assert!(sotto_il_primo.opzioni_superblocco.contains("nsdelegate"));
    }

    /// Il confronto e' per **componenti**, non per testo.
    ///
    /// `/sys/fs/cgroup2` ha come prefisso testuale `/sys/fs/cgroup`, ma non e'
    /// dentro quella gerarchia: un `starts_with` su stringhe lo accetterebbe, e
    /// il dominio verrebbe attribuito al montaggio sbagliato.
    #[test]
    fn il_prefisso_e_per_componenti_non_per_testo() {
        let mountinfo = "29 23 0:27 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n";
        assert!(montaggio_che_contiene(mountinfo, Path::new("/sys/fs/cgroup2/x")).is_err());
        assert!(montaggio_che_contiene(mountinfo, Path::new("/sys/fs/cgroup/x")).is_ok());
    }

    /// Le opzioni del **mount** e quelle del **superblocco** sono due campi
    /// diversi, e confonderli e' un errore silenzioso.
    ///
    /// `memory_localevents` governa il superblocco, quindi il kernel la riporta
    /// nell'ultimo campo. Cercarla in quello del mount rende sempre «assente»,
    /// e l'assenza e' proprio la risposta che fa proseguire: il difetto non si
    /// manifesterebbe mai come un rifiuto, solo come una registrazione falsa.
    #[test]
    fn le_opzioni_del_mount_non_sono_quelle_del_superblocco() {
        let solo_nel_mount =
            "29 23 0:27 / /sys/fs/cgroup rw,memory_localevents - cgroup2 cgroup2 rw,nsdelegate\n";
        let montaggio = montaggio_che_contiene(solo_nel_mount, Path::new("/sys/fs/cgroup/x"))
            .expect("montaggio");
        assert!(montaggio.opzioni_mount.contains("memory_localevents"));
        assert!(
            !montaggio.opzioni_superblocco.contains("memory_localevents"),
            "nel campo del mount non conta"
        );

        let nel_superblocco =
            "29 23 0:27 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw,memory_localevents\n";
        let montaggio = montaggio_che_contiene(nel_superblocco, Path::new("/sys/fs/cgroup/x"))
            .expect("montaggio");
        assert!(montaggio.opzioni_superblocco.contains("memory_localevents"));
    }

    /// Il dispositivo si conserva.
    #[test]
    fn il_dispositivo_torna_nell_esito() {
        let montaggio =
            montaggio_che_contiene(DUE_MOUNT, Path::new("/mnt/dominio")).expect("montaggio");
        assert_eq!(montaggio.dispositivo, "0:27");
    }

    /// Nessun `cgroup2` che contenga il percorso e' un rifiuto.
    #[test]
    fn nessun_montaggio_che_contiene_e_un_rifiuto() {
        assert!(montaggio_che_contiene(DUE_MOUNT, Path::new("/tmp/altrove"))
            .expect_err("nessuno lo contiene")
            .to_string()
            .contains("nessun cgroup2"));
    }

    /// Due montaggi allo stesso punto sono ambigui.
    #[test]
    fn due_montaggi_allo_stesso_punto_sono_un_rifiuto() {
        let ambiguo = "\
29 23 0:27 / /sys/fs/cgroup rw shared:9 - cgroup2 cgroup2 rw,nsdelegate
30 23 0:31 / /sys/fs/cgroup rw shared:9 - cgroup2 cgroup2 rw,memory_localevents
";
        assert!(
            montaggio_che_contiene(ambiguo, Path::new("/sys/fs/cgroup/x"))
                .expect_err("due allo stesso livello")
                .to_string()
                .contains("2 montaggi")
        );
    }

    /// Il tipo si legge **dopo il trattino**, non a una posizione fissa.
    #[test]
    fn i_campi_opzionali_non_spostano_il_tipo() {
        let con_molti = "\
29 23 0:27 / /sys/fs/cgroup rw shared:9 master:3 propagate_from:2 unbindable - cgroup2 cgroup2 rw
";
        assert!(montaggio_che_contiene(con_molti, Path::new("/sys/fs/cgroup/x")).is_ok());
        let senza = "29 23 0:27 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n";
        assert!(montaggio_che_contiene(senza, Path::new("/sys/fs/cgroup/x")).is_ok());
    }

    /// Un filesystem che **non** e' `cgroup2` non e' il nostro, per quanto la
    /// sorgente si chiami cosi'.
    #[test]
    fn il_tipo_conta_non_il_nome_della_sorgente() {
        let finto = "29 23 0:27 / /sys/fs/cgroup rw - ext4 cgroup2 rw\n";
        assert!(montaggio_che_contiene(finto, Path::new("/sys/fs/cgroup/x")).is_err());
    }

    /// Una riga che non si interpreta **ferma** il parsing: non si salta.
    ///
    /// Saltarla vorrebbe dire scegliere un montaggio avendo ignorato proprio
    /// la riga che non si capisce, che potrebbe essere quella giusta.
    #[test]
    fn una_riga_malformata_ferma_il_parsing() {
        let con_riga_rotta = format!("rotta\n{DUE_MOUNT}");
        let difetto = montaggio_che_contiene(&con_riga_rotta, Path::new("/mnt/dominio"))
            .expect_err("una riga rotta non si salta");
        assert!(difetto.to_string().contains("riga 1"), "{difetto}");

        // Una riga vuota invece si salta: e' la fine del file, non una forma
        // che non si capisce.
        let con_riga_vuota = format!("{DUE_MOUNT}\n");
        assert!(montaggio_che_contiene(&con_riga_vuota, Path::new("/mnt/dominio")).is_ok());
    }

    /// Gli escape ottali si decodificano, e solo i quattro dichiarati.
    #[test]
    fn gli_escape_sono_quattro_e_non_di_piu() {
        assert_eq!(
            decodifica(r"/mnt/con\040spazio").expect("spazio"),
            PathBuf::from("/mnt/con spazio")
        );
        assert_eq!(
            decodifica(r"/mnt/con\134barra").expect("barra"),
            PathBuf::from(r"/mnt/con\barra")
        );
        assert!(
            decodifica(r"/mnt/\101").is_err(),
            "\\101 e' una A, ma il kernel non la emette cosi': interpretarla sarebbe inventare"
        );
        assert!(decodifica(r"/mnt/rotto\9").is_err(), "escape troncato");
    }

    /// I byte non ASCII passano **identici**.
    ///
    /// Decodificare carattere per carattere spezzerebbe le sequenze multibyte e
    /// le ricomporrebbe sbagliate: un percorso con un accento diventerebbe un
    /// percorso diverso, e il confronto per prefisso fallirebbe in silenzio.
    #[test]
    fn i_percorsi_non_ascii_restano_quelli() {
        let con_accento = "/mnt/città/dominio";
        assert_eq!(
            decodifica(con_accento).expect("accento"),
            PathBuf::from(con_accento)
        );
        let misto = r"/mnt/città\040con spazio";
        assert_eq!(
            decodifica(misto).expect("misto"),
            PathBuf::from("/mnt/città con spazio")
        );
    }
}
