//! Deadline con clock controllati e valori oltre il calendario.
use plenora_core::{tempo, ErrorCategory, ErrorPhase};
use std::time::{Duration, Instant, SystemTime};
#[macro_use]
#[path = "../../../tests/comune/esito.rs"]
mod esito;

#[test]
fn deadline_unix_limiti_e_precisione() {
    let now = Instant::now();
    let epoch = SystemTime::UNIX_EPOCH;
    for seconds in [i64::MIN, -62_135_596_800, -11_644_473_600, -1, 0] {
        assert_successo!(tempo::deadline_unix(seconds, 0, epoch, now), now);
    }
    // Anno 9999 e anno 10000: nessun restringimento al calendario RFC3339.
    for seconds in [253_402_300_799_i64, 253_402_300_800] {
        let expected = now
            .checked_add(Duration::from_secs(seconds.cast_unsigned()))
            .unwrap();
        assert_successo!(tempo::deadline_unix(seconds, 0, epoch, now), expected);
    }
    let expected = now.checked_add(Duration::new(1, 123_456_700)).unwrap();
    assert_successo!(tempo::deadline_unix(1, 123_456_700, epoch, now), expected);
    assert_rifiuto!(
        tempo::deadline_unix(1, 1_000_000_000, epoch, now),
        ErrorCategory::InvalidConfiguration,
        ErrorPhase::Prepare
    );
    assert_rifiuto!(
        tempo::timeout(now, Duration::MAX),
        ErrorCategory::InvalidConfiguration,
        ErrorPhase::Prepare
    );
}

#[test]
fn systemtime_oltre_rfc3339_e_pre_epoca() {
    let now = Instant::now();
    // Oltre il dominio RFC3339 a quattro cifre, anche su Windows.
    let distant = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_hours(175_316_400))
        .unwrap();
    assert_successo!(
        tempo::deadline_unix(253_402_300_799, 999_999_999, distant, now),
        now
    );
    let before = SystemTime::UNIX_EPOCH
        .checked_sub(Duration::from_secs(1))
        .unwrap();
    assert_successo!(tempo::deadline_unix(-1, 0, before, now), now);
    assert_successo!(
        tempo::deadline_unix(-1, 500_000_000, before, now),
        now.checked_add(Duration::from_millis(500)).unwrap()
    );
    assert_successo!(
        tempo::deadline_systemtime(before, SystemTime::UNIX_EPOCH, now),
        now
    );
}

#[test]
fn precisione_frazioni_e_float_sdk() {
    for text in [
        "9999-01-01T00:00:00.123456789Z",
        "0001-01-01T00:00:00.1234567890Z",
    ] {
        assert_successo!(tempo::frazione_rfc3339_esatta(text), ());
    }
    assert_rifiuto!(
        tempo::frazione_rfc3339_esatta("0001-01-01T00:00:00.1234567891Z"),
        ErrorCategory::InvalidConfiguration,
        ErrorPhase::Prepare
    );
    let now = Instant::now();
    for value in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert_rifiuto!(
            tempo::timeout_secondi_f64(now, value),
            ErrorCategory::InvalidConfiguration,
            ErrorPhase::Prepare
        );
    }
    for value in [-0.0, 0.0, f64::from_bits(1)] {
        assert_successo!(tempo::timeout_secondi_f64(now, value), now);
    }
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
        assert_rifiuto!(
            tempo::deadline_secondi_f64(value, SystemTime::UNIX_EPOCH, now),
            ErrorCategory::InvalidConfiguration,
            ErrorPhase::Prepare
        );
    }
    for value in [-0.0, 0.0, -1.0, -62_135_596_800.0, f64::from_bits(1)] {
        assert_successo!(
            tempo::deadline_secondi_f64(value, SystemTime::UNIX_EPOCH, now),
            now
        );
    }
    assert_successo!(
        tempo::deadline_secondi_f64(1.5, SystemTime::UNIX_EPOCH, now),
        now.checked_add(Duration::from_millis(1500)).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn systemtime_oltre_chrono_su_unix() {
    let now = Instant::now();
    let distant = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_hours(8_760_000_000))
        .unwrap();
    assert_successo!(tempo::deadline_unix(253_402_300_799, 0, distant, now), now);
}
