# -*- coding: utf-8 -*-
"""Rimuove i profili grezzi di `cargo llvm-cov` prima e dopo la misura.

# Perche' serve

`cargo llvm-cov` fa scrivere ai processi strumentati un file per profilo, con
un nome della forma `work-<pid>-<firma>_0.profraw`. Il pid viene **riciclato**
dal sistema operativo e la firma del modulo e' costante per lo stesso binario:
se nella directory sono rimasti i profili di esecuzioni precedenti, un
processo nuovo puo' trovare il proprio nome gia' occupato. LLVM allora scrive
su **stderr**:

    LLVM Profile Error: Profile Merging of file ... failed: File exists

e ogni test che pretende stderr vuoto — cioe' i golden di canale della CLI —
fallisce per una ragione che non ha niente a che vedere con il codice.

Non e' teoria: con qualche migliaio di `.profraw` accumulati in `target-cov`
campagne consecutive falliscono, e dopo la pulizia gli errori LLVM spariscono
e la coverage torna sopra tutte le soglie. Su una macchina di sviluppo la
directory si riempie in poche decine di esecuzioni; in CI la stessa cosa
succede piu' lentamente, perche' `target-cov` **e' in cache** e la cache si
conserva finche' non cambia `Cargo.lock`.

Un fallimento del genere non si legge: sembra un test rotto, non un residuo.

# Che cosa fa, e che cosa non fa

Cancella **soltanto** i file `*.profraw` sotto la directory di coverage. Non
tocca gli artefatti compilati, i report `lcov`/`json`, le directory. Il
percorso e' validato prima di qualunque cancellazione: dev'essere dentro la
radice del repository e chiamarsi come la directory di coverage attesa.

    python scripts/pulisci_profili_coverage.py [PERCORSO]

Le regressioni girano a ogni invocazione: costano microsecondi, e uno script
che cancella file va visto sbagliare almeno una volta prima di fidarsene.
"""
import io
import os
import shutil
import sys
import tempfile

# Il nome atteso della directory di coverage. Un percorso che non finisce
# cosi' viene rifiutato: questo script cancella file, e cancellare nel posto
# sbagliato e' un danno che non si annulla.
NOME_ATTESO = 'target-cov'
ESTENSIONE = '.profraw'

RADICE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class PercorsoNonAmmesso(Exception):
    """Il percorso indicato non e' una directory di coverage di questo repo."""


class ScansioneFallita(Exception):
    """Una parte della directory non e' stata leggibile.

    `os.walk` senza `onerror` **ignora** gli errori: una sottodirectory che
    non si riesce ad aprire viene semplicemente saltata, e la funzione
    restituisce come se avesse visto tutto. Qui significherebbe dichiarare
    puliti dei profili che sono ancora sul disco, e il primo a scoprirlo
    sarebbe un test rosso in una campagna successiva.
    """


def _valida(percorso, radice):
    """Il percorso assoluto della directory, o un'eccezione.

    Tre condizioni, tutte necessarie: dev'essere dentro `radice`, deve
    chiamarsi `target-cov`, e non dev'essere un collegamento simbolico — un
    symlink dentro la radice puo' puntare fuori, e la verifica di
    appartenenza guarderebbe il posto sbagliato.
    """
    assoluto = os.path.realpath(percorso)
    radice = os.path.realpath(radice)
    if os.path.basename(assoluto) != NOME_ATTESO:
        raise PercorsoNonAmmesso(
            "atteso un percorso che finisca in %r, ricevuto %r"
            % (NOME_ATTESO, percorso))
    comune = os.path.commonpath([assoluto, radice])
    if comune != radice or assoluto == radice:
        raise PercorsoNonAmmesso(
            "%r e' fuori dalla radice del repository (%r)" % (percorso, radice))
    if os.path.islink(percorso):
        raise PercorsoNonAmmesso(
            "%r e' un collegamento simbolico: non si cancella attraverso un "
            "symlink" % percorso)
    return assoluto


def pulisci(percorso, radice=RADICE):
    """Cancella i `*.profraw` sotto `percorso`. Restituisce quanti.

    Se la directory non esiste non e' un errore: la prima misura parte da
    zero, e pretendere che esista renderebbe lo script inutilizzabile proprio
    al primo giro.
    """
    assoluto = _valida(percorso, radice)
    if not os.path.isdir(assoluto):
        return 0

    def propaga(errore):
        raise ScansioneFallita(
            'directory non leggibile durante la scansione: %s' % errore)

    rimossi = 0
    for cartella, _, nomi in os.walk(assoluto, onerror=propaga):
        for nome in nomi:
            if nome.endswith(ESTENSIONE):
                os.remove(os.path.join(cartella, nome))
                rimossi += 1
    return rimossi


def autotest():
    """Regressioni. Girano sempre: cancella file, va visto sbagliare."""
    base = tempfile.mkdtemp(prefix='pulisci-profili-')
    try:
        radice = os.path.join(base, 'repo')
        coverage = os.path.join(radice, NOME_ATTESO)
        annidata = os.path.join(coverage, 'llvm-cov-target', 'deps')
        os.makedirs(annidata)

        # Profili stantii, a due livelli di profondita'.
        stantii = [
            os.path.join(coverage, 'work-1-aaa_0' + ESTENSIONE),
            os.path.join(coverage, 'llvm-cov-target', 'work-2-bbb_0' + ESTENSIONE),
            os.path.join(annidata, 'work-3-ccc_0' + ESTENSIONE),
        ]
        # Tutto il resto deve sopravvivere: report, artefatti, un file il cui
        # NOME contiene l'estensione senza finire con essa.
        da_preservare = [
            os.path.join(coverage, 'lcov.info'),
            os.path.join(coverage, 'report.json'),
            os.path.join(annidata, 'libplenora_core.rlib'),
            os.path.join(annidata, 'work-4' + ESTENSIONE + '.bak'),
        ]
        for percorso in stantii + da_preservare:
            io.open(percorso, 'w', encoding='utf-8').write('x')

        rimossi = pulisci(coverage, radice)
        if rimossi != len(stantii):
            raise SystemExit(
                'autotest: rimossi %d profili invece di %d'
                % (rimossi, len(stantii)))
        rimasti = [p for p in stantii if os.path.exists(p)]
        if rimasti:
            raise SystemExit('autotest: profili stantii sopravvissuti: %r'
                             % rimasti)
        persi = [p for p in da_preservare if not os.path.exists(p)]
        if persi:
            raise SystemExit(
                'autotest: cancellati file che andavano preservati: %r.\n'
                '  Uno script che cancella piu' + "'" + ' del dovuto e\' peggio '
                'di uno che non cancella.' % persi)
        if not os.path.isdir(annidata):
            raise SystemExit('autotest: cancellata una directory')

        # Una seconda passata su una directory gia' pulita non e' un errore.
        if pulisci(coverage, radice) != 0:
            raise SystemExit('autotest: la seconda passata ha rimosso qualcosa')

        # Directory assente: zero, non eccezione.
        assente = os.path.join(radice, 'altro', NOME_ATTESO)
        if pulisci(assente, radice) != 0:
            raise SystemExit('autotest: directory assente non gestita')

        # Percorso fuori dalla radice.
        fuori = os.path.join(base, NOME_ATTESO)
        os.makedirs(fuori)
        try:
            pulisci(fuori, radice)
        except PercorsoNonAmmesso:
            pass
        else:
            raise SystemExit(
                'autotest: accettato un percorso fuori dalla radice')

        # Nome sbagliato: non basta essere dentro la radice.
        sbagliato = os.path.join(radice, 'target')
        os.makedirs(sbagliato)
        try:
            pulisci(sbagliato, radice)
        except PercorsoNonAmmesso:
            pass
        else:
            raise SystemExit('autotest: accettata una directory con nome %r'
                             % os.path.basename(sbagliato))

        # La radice stessa non e' una directory di coverage.
        try:
            pulisci(radice, radice)
        except PercorsoNonAmmesso:
            pass
        else:
            raise SystemExit('autotest: accettata la radice del repository')

        _autotest_scansione(coverage, radice)
    finally:
        shutil.rmtree(base, ignore_errors=True)


def _autotest_scansione(coverage, radice):
    """Un errore di scansione deve fermare la pulizia, non essere saltato.

    Due prove. La prima e' portabile: si sostituisce `os.walk` con uno stub
    che invoca `onerror`, e si pretende che l'eccezione arrivi fino al
    chiamante — verifica che il callback sia passato e che propaghi davvero.
    La seconda e' reale ma solo su POSIX: una sottodirectory senza permesso
    di lettura. Su Windows i permessi non si tolgono al proprietario in modo
    affidabile, e da root il permesso non si applica: li' si salta, ed e'
    detto qui invece di essere taciuto.
    """
    def walk_che_fallisce(top, onerror=None, **_ignorati):
        if onerror is not None:
            onerror(OSError(13, 'permesso negato', top))
        return iter(())

    originale = os.walk
    os.walk = walk_che_fallisce
    try:
        pulisci(coverage, radice)
    except ScansioneFallita:
        pass
    except Exception as errore:  # noqa: BLE001 - l'autotest deve dire quale
        raise SystemExit('autotest: errore di scansione riportato come %r'
                         % errore)
    else:
        raise SystemExit(
            'autotest: un errore di scansione e\' stato ignorato. `os.walk` '
            'senza `onerror` salta le directory illeggibili e dichiara '
            'successo: i profili resterebbero sul disco.')
    finally:
        os.walk = originale

    if os.name == 'posix' and hasattr(os, 'geteuid') and os.geteuid() != 0:
        chiusa = os.path.join(coverage, 'chiusa')
        os.makedirs(chiusa, exist_ok=True)
        io.open(os.path.join(chiusa, 'work-9-zzz' + ESTENSIONE),
                'w', encoding='utf-8').write('x')
        os.chmod(chiusa, 0o000)
        try:
            pulisci(coverage, radice)
        except ScansioneFallita:
            pass
        else:
            raise SystemExit(
                'autotest: una sottodirectory illeggibile non ha fermato la '
                'pulizia')
        finally:
            os.chmod(chiusa, 0o700)


def main(argomenti):
    if len(argomenti) > 1:
        print('uso: pulisci_profili_coverage.py [PERCORSO]')
        return 2
    autotest()
    percorso = argomenti[0] if argomenti else os.path.join(RADICE, NOME_ATTESO)
    try:
        rimossi = pulisci(percorso)
    except (PercorsoNonAmmesso, ScansioneFallita) as errore:
        print('ERRORE: %s' % errore)
        return 1
    print('profili di coverage rimossi: %d (%s)' % (rimossi, percorso))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
