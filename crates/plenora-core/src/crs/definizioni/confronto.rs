//! Confronto completo. Gli insiemi sono piccoli e si cercano senza hash;
//! nessun ordinamento modifica gli assi o le coordinate del chiamante.

use super::numeri::uguali;
use super::{Base, Conversione, Datum, Definizione, Id, Identita, Rifiuto, Sistema, Unita};

fn identita(a: &Identita, b: &Identita) -> bool {
    a.id.as_ref()
        .map_or_else(|| a.nome == b.nome, |id| Some(id) == b.id.as_ref())
}

fn id_opzionale(a: Option<&Id>, b: Option<&Id>) -> bool {
    a.is_none_or(|id| Some(id) == b)
}

fn unita(a: &Unita, b: &Unita) -> Result<bool, Rifiuto> {
    Ok(identita(&a.identita, &b.identita) && a.tipo == b.tipo && uguali(&a.fattore, &b.fattore)?)
}

fn sistema(a: &Sistema, b: &Sistema) -> Result<bool, Rifiuto> {
    if a.tipo != b.tipo || a.assi.len() != b.assi.len() {
        return Ok(false);
    }
    for (a, b) in a.assi.iter().zip(&b.assi) {
        if a.direzione != b.direzione || a.ordine != b.ordine || !unita(&a.unita, &b.unita)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn datum(a: &Datum, b: &Datum) -> Result<bool, Rifiuto> {
    if !identita(&a.identita, &b.identita) || a.tipo != b.tipo || a.membri.len() != b.membri.len() {
        return Ok(false);
    }
    // Ogni membro atteso compare una sola volta: anche duplicati ed
    // eccedenti sono rifiutati, indipendentemente dall'ordine di scrittura.
    for membro in &b.membri {
        if a.membri
            .iter()
            .filter(|voce| identita(voce, membro))
            .count()
            != 1
        {
            return Ok(false);
        }
    }
    let accuratezza = match (&a.accuratezza, &b.accuratezza) {
        (None, None) => true,
        (Some(a), Some(b)) => uguali(a, b)?,
        _ => false,
    };
    let ea = &a.ellissoide;
    let eb = &b.ellissoide;
    let ma = &a.meridiano;
    let mb = &b.meridiano;
    Ok(accuratezza
        && identita(&ea.identita, &eb.identita)
        && uguali(&ea.semiasse, &eb.semiasse)?
        && ea.forma == eb.forma
        && uguali(&ea.seconda_misura, &eb.seconda_misura)?
        && unita(&ea.unita, &eb.unita)?
        && identita(&ma.identita, &mb.identita)
        && uguali(&ma.longitudine, &mb.longitudine)?
        && unita(&ma.unita, &mb.unita)?)
}

fn base(a: &Base, b: &Base) -> Result<bool, Rifiuto> {
    Ok(id_opzionale(a.id.as_ref(), b.id.as_ref())
        && datum(&a.datum, &b.datum)?
        && sistema(&a.sistema, &b.sistema)?)
}

fn conversione(a: &Conversione, b: &Conversione) -> Result<bool, Rifiuto> {
    if !id_opzionale(a.id.as_ref(), b.id.as_ref())
        || !identita(&a.metodo, &b.metodo)
        || a.parametri.len() != b.parametri.len()
    {
        return Ok(false);
    }
    for atteso in &b.parametri {
        let mut candidati = a
            .parametri
            .iter()
            .filter(|p| identita(&p.identita, &atteso.identita));
        let Some(trovato) = candidati.next() else {
            return Ok(false);
        };
        if candidati.next().is_some()
            || !uguali(&trovato.valore, &atteso.valore)?
            || !unita(&trovato.unita, &atteso.unita)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn confronta(a: &Definizione, b: &Definizione) -> Result<(), Rifiuto> {
    let conversione = match (&a.conversione, &b.conversione) {
        (None, None) => true,
        (Some(a), Some(b)) => conversione(a, b)?,
        _ => false,
    };
    if a.id == b.id
        && a.tipo == b.tipo
        && base(&a.base, &b.base)?
        && conversione
        && sistema(&a.sistema, &b.sistema)?
    {
        Ok(())
    } else {
        Err(Rifiuto::NonIntegrato)
    }
}
